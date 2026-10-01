//! Mensajes intercambiados entre la GUI y el worker BLE.

use btleplug::api::CharPropFlags;
use btleplug::platform::PeripheralId;
use uuid::Uuid;

/// Referencia a una característica concreta (servicio + característica).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CharRef {
    pub service: Uuid,
    pub uuid: Uuid,
}

/// Características activas para escribir y para recibir notificaciones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub write: Option<CharRef>,
    pub notify: Option<CharRef>,
}

#[derive(Clone, Debug)]
pub struct CharInfo {
    pub uuid: Uuid,
    pub props: CharPropFlags,
}

impl CharInfo {
    pub fn can_write(&self) -> bool {
        self.props
            .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
    }

    pub fn can_notify(&self) -> bool {
        self.props
            .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
    }
}

#[derive(Clone, Debug)]
pub struct ServiceInfo {
    pub uuid: Uuid,
    pub chars: Vec<CharInfo>,
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: PeripheralId,
    pub name: Option<String>,
    pub address: String,
    pub rssi: Option<i16>,
    pub has_nus: bool,
}

/// GUI -> worker BLE.
#[derive(Debug)]
pub enum BleCommand {
    StartScan,
    StopScan,
    Connect {
        id: PeripheralId,
        prefer_write: Uuid,
        prefer_notify: Uuid,
    },
    Disconnect,
    Select(Selection),
    Send {
        data: Vec<u8>,
        /// 0 = automático según MTU.
        chunk_size: usize,
        with_response: bool,
    },
}

/// Worker BLE -> GUI.
#[derive(Debug)]
pub enum BleEvent {
    Info(String),
    Error(String),
    ScanState(bool),
    Device(DeviceInfo),
    Connecting(String),
    Connected { name: String, mtu: u16 },
    Services(Vec<ServiceInfo>),
    Selected(Selection),
    Data(Vec<u8>),
    Sent(Vec<u8>),
    Disconnected,
}
