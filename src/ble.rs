//! Worker BLE: corre en un runtime tokio propio y se comunica con la GUI por canales.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use btleplug::api::{
    Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use eframe::egui;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::config::{NUS_RX_WRITE, NUS_SERVICE, NUS_TX_NOTIFY, effective_chunk_size};
use crate::messages::{
    BleCommand, BleEvent, CharInfo, CharRef, DeviceInfo, Selection, ServiceInfo,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(20);

/// Envía eventos a la GUI y la despierta para que repinte.
#[derive(Clone)]
struct EventTx {
    tx: std::sync::mpsc::Sender<BleEvent>,
    ctx: egui::Context,
}

impl EventTx {
    fn send(&self, ev: BleEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }

    fn error(&self, msg: impl Into<String>) {
        self.send(BleEvent::Error(msg.into()));
    }
}

/// Lanza el worker en un hilo aparte.
pub fn spawn(
    ctx: egui::Context,
) -> (
    UnboundedSender<BleCommand>,
    std::sync::mpsc::Receiver<BleEvent>,
) {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();
    let events = EventTx { tx: evt_tx, ctx };
    std::thread::Builder::new()
        .name("ble-worker".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    events.error(format!("No se pudo crear el runtime: {e}"));
                    return;
                }
            };
            rt.block_on(Worker::run(cmd_rx, events));
        })
        .expect("no se pudo lanzar el hilo BLE");
    (cmd_tx, evt_rx)
}

struct Connection {
    peripheral: Peripheral,
    write: Option<Characteristic>,
    notify: Option<Characteristic>,
    /// UUID de la característica cuyas notificaciones se reenvían.
    notify_filter: Arc<Mutex<Option<Uuid>>>,
    notif_task: JoinHandle<()>,
}

struct Worker {
    adapter: Adapter,
    events: EventTx,
    conn: Option<Connection>,
    scanning: bool,
}

impl Worker {
    async fn run(mut cmd_rx: UnboundedReceiver<BleCommand>, events: EventTx) {
        let adapter = match first_adapter().await {
            Ok(a) => a,
            Err(e) => {
                events.error(format!("Bluetooth no disponible: {e:#}"));
                // Mantener el canal vivo informando del error en cada comando.
                while cmd_rx.recv().await.is_some() {
                    events.error("Bluetooth no disponible");
                }
                return;
            }
        };
        if let Ok(info) = adapter.adapter_info().await {
            events.send(BleEvent::Info(format!("Adaptador: {info}")));
        }
        let mut central_events = match adapter.events().await {
            Ok(s) => s,
            Err(e) => {
                events.error(format!("No se pudieron obtener eventos del adaptador: {e}"));
                return;
            }
        };

        let mut w = Worker {
            adapter,
            events,
            conn: None,
            scanning: false,
        };

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => {
                        if let Err(e) = w.handle_command(cmd).await {
                            w.events.error(format!("{e:#}"));
                        }
                    }
                    None => break, // GUI cerrada
                },
                Some(ev) = central_events.next() => w.handle_central(ev).await,
            }
        }
        w.disconnect(true).await;
        if w.scanning {
            let _ = w.adapter.stop_scan().await;
        }
    }

    async fn handle_command(&mut self, cmd: BleCommand) -> Result<()> {
        match cmd {
            BleCommand::StartScan => {
                self.adapter
                    .start_scan(ScanFilter::default())
                    .await
                    .context("no se pudo iniciar el escaneo")?;
                self.scanning = true;
                self.events.send(BleEvent::ScanState(true));
                // Reportar los ya conocidos (útil en Linux, donde BlueZ cachea).
                for p in self.adapter.peripherals().await.unwrap_or_default() {
                    self.report_device(&p.id()).await;
                }
            }
            BleCommand::StopScan => self.stop_scan().await,
            BleCommand::Connect {
                id,
                prefer_write,
                prefer_notify,
            } => {
                self.disconnect(true).await;
                self.stop_scan().await;
                self.connect(&id, prefer_write, prefer_notify).await?;
            }
            BleCommand::Disconnect => self.disconnect(true).await,
            BleCommand::Select(sel) => self.select(sel).await?,
            BleCommand::Send {
                data,
                chunk_size,
                with_response,
            } => self.send(data, chunk_size, with_response).await?,
        }
        Ok(())
    }

    async fn handle_central(&mut self, ev: CentralEvent) {
        match ev {
            CentralEvent::DeviceDiscovered(id)
            | CentralEvent::DeviceUpdated(id)
            | CentralEvent::RssiUpdate { id, .. }
            | CentralEvent::ServicesAdvertisement { id, .. } => {
                if self.scanning {
                    self.report_device(&id).await;
                }
            }
            CentralEvent::DeviceDisconnected(id)
                if self.conn.as_ref().is_some_and(|c| c.peripheral.id() == id) =>
            {
                self.disconnect(false).await;
            }
            _ => {}
        }
    }

    async fn report_device(&self, id: &PeripheralId) {
        let Ok(p) = self.adapter.peripheral(id).await else {
            return;
        };
        let Ok(Some(props)) = p.properties().await else {
            return;
        };
        self.events.send(BleEvent::Device(DeviceInfo {
            id: id.clone(),
            name: props
                .local_name
                .or(props.advertisement_name)
                .filter(|n| !n.trim().is_empty()),
            address: props.address.to_string(),
            rssi: props.rssi,
            has_nus: props.services.contains(&NUS_SERVICE),
        }));
    }

    async fn stop_scan(&mut self) {
        if self.scanning {
            let _ = self.adapter.stop_scan().await;
            self.scanning = false;
            self.events.send(BleEvent::ScanState(false));
        }
    }

    async fn connect(
        &mut self,
        id: &PeripheralId,
        prefer_write: Uuid,
        prefer_notify: Uuid,
    ) -> Result<()> {
        let p = self
            .adapter
            .peripheral(id)
            .await
            .context("dispositivo desconocido")?;
        let name = p
            .properties()
            .await
            .ok()
            .flatten()
            .and_then(|pr| pr.local_name.or(pr.advertisement_name))
            .unwrap_or_else(|| p.address().to_string());
        self.events.send(BleEvent::Connecting(name.clone()));

        let result: Result<()> = async {
            if !p.is_connected().await.unwrap_or(false) {
                tokio::time::timeout(CONNECT_TIMEOUT, p.connect())
                    .await
                    .map_err(|_| anyhow!("tiempo de conexión agotado"))?
                    .map_err(|e| match e {
                        // WinRT no distingue "fuera de alcance" de "no conectable".
                        btleplug::Error::DeviceNotFound => {
                            anyhow!("dispositivo no encontrado (fuera de alcance o no conectable)")
                        }
                        e => anyhow!(e),
                    })
                    .context("fallo al conectar")?;
            }
            tokio::time::timeout(DISCOVER_TIMEOUT, p.discover_services())
                .await
                .map_err(|_| anyhow!("tiempo de descubrimiento agotado"))?
                .context("fallo al descubrir servicios")?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = p.disconnect().await;
            self.events.send(BleEvent::Disconnected);
            return Err(e);
        }

        // Reenvío de notificaciones filtradas por la característica seleccionada.
        let notify_filter = Arc::new(Mutex::new(None));
        let mut stream = p
            .notifications()
            .await
            .context("sin flujo de notificaciones")?;
        let filter = notify_filter.clone();
        let events = self.events.clone();
        let notif_task = tokio::spawn(async move {
            while let Some(n) = stream.next().await {
                if *filter.lock().unwrap() == Some(n.uuid) {
                    events.send(BleEvent::Data(n.value));
                }
            }
        });

        let services: Vec<ServiceInfo> = p
            .services()
            .into_iter()
            .map(|s| ServiceInfo {
                uuid: s.uuid,
                chars: s
                    .characteristics
                    .into_iter()
                    .map(|c| CharInfo {
                        uuid: c.uuid,
                        props: c.properties,
                    })
                    .collect(),
            })
            .collect();

        self.events.send(BleEvent::Connected { name, mtu: p.mtu() });
        self.events.send(BleEvent::Services(services));

        let chars: Vec<Characteristic> = p.characteristics().into_iter().collect();
        self.conn = Some(Connection {
            peripheral: p,
            write: None,
            notify: None,
            notify_filter,
            notif_task,
        });
        self.select(auto_select(&chars, prefer_write, prefer_notify))
            .await
    }

    async fn select(&mut self, sel: Selection) -> Result<()> {
        let conn = self.conn.as_mut().context("no hay conexión")?;
        let chars = conn.peripheral.characteristics();
        let find = |r: Option<CharRef>| {
            r.and_then(|r| {
                chars
                    .iter()
                    .find(|c| c.uuid == r.uuid && c.service_uuid == r.service)
                    .cloned()
            })
        };
        let new_write = find(sel.write);
        let new_notify = find(sel.notify);

        if conn.notify != new_notify {
            if let Some(old) = conn.notify.take() {
                let _ = conn.peripheral.unsubscribe(&old).await;
            }
            *conn.notify_filter.lock().unwrap() = None;
            if let Some(n) = &new_notify {
                conn.peripheral
                    .subscribe(n)
                    .await
                    .with_context(|| format!("no se pudo suscribir a {}", n.uuid))?;
                *conn.notify_filter.lock().unwrap() = Some(n.uuid);
            }
            conn.notify = new_notify;
        }
        conn.write = new_write;

        let to_ref = |c: &Characteristic| CharRef {
            service: c.service_uuid,
            uuid: c.uuid,
        };
        self.events.send(BleEvent::Selected(Selection {
            write: conn.write.as_ref().map(to_ref),
            notify: conn.notify.as_ref().map(to_ref),
        }));
        Ok(())
    }

    async fn send(&mut self, data: Vec<u8>, chunk_size: usize, with_response: bool) -> Result<()> {
        let conn = self.conn.as_ref().context("no hay conexión")?;
        let ch = conn
            .write
            .as_ref()
            .context("no hay característica de escritura seleccionada")?;
        let supports_wo = ch
            .properties
            .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE);
        let supports_w = ch.properties.contains(CharPropFlags::WRITE);
        let wt = if (with_response && supports_w) || !supports_wo {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        let size = effective_chunk_size(chunk_size, conn.peripheral.mtu());
        for chunk in data.chunks(size) {
            conn.peripheral
                .write(ch, chunk, wt)
                .await
                .context("fallo al escribir")?;
        }
        self.events.send(BleEvent::Sent(data));
        Ok(())
    }

    /// Limpia el estado de conexión. Con `physical` además cierra el enlace BLE
    /// (sin él, sólo se reacciona a una desconexión ya ocurrida).
    async fn disconnect(&mut self, physical: bool) {
        if let Some(c) = self.conn.take() {
            c.notif_task.abort();
            if physical {
                if let Some(n) = &c.notify {
                    let _ = c.peripheral.unsubscribe(n).await;
                }
                let _ = c.peripheral.disconnect().await;
            }
            self.events.send(BleEvent::Disconnected);
        }
    }
}

async fn first_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no se encontró ningún adaptador Bluetooth"))
}

/// Elige características: preferidas → NUS → primera que sirva.
fn auto_select(chars: &[Characteristic], prefer_write: Uuid, prefer_notify: Uuid) -> Selection {
    let pick = |uuids: [Uuid; 2], ok: fn(CharPropFlags) -> bool| {
        uuids
            .iter()
            .find_map(|u| chars.iter().find(|c| c.uuid == *u && ok(c.properties)))
            .or_else(|| chars.iter().find(|c| ok(c.properties)))
            .map(|c| CharRef {
                service: c.service_uuid,
                uuid: c.uuid,
            })
    };
    Selection {
        write: pick([prefer_write, NUS_RX_WRITE], |p| {
            p.intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
        }),
        notify: pick([prefer_notify, NUS_TX_NOTIFY], |p| {
            p.intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
        }),
    }
}
