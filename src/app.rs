//! Interfaz gráfica egui.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use btleplug::api::CharPropFlags;
use btleplug::platform::PeripheralId;
use chrono::{DateTime, Local};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Key, Layout, Margin, RichText, Sense, Stroke, vec2,
};
use egui_phosphor::regular as icon;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::config::{
    self, Config, LineEnding, Macro, MacroMode, NUS_RX_WRITE, NUS_SERVICE, NUS_TX_NOTIFY,
};
use crate::log::SessionLog;
use crate::messages::{BleCommand, BleEvent, CharRef, DeviceInfo, Selection, ServiceInfo};
use crate::theme::{self, Palette, mix};

const MAX_LINES: usize = 20_000;
const MAX_HISTORY: usize = 100;
/// Valor inicial del filtro de RSSI al activarlo.
const DEFAULT_MIN_RSSI: i16 = -80;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Rx,
    Tx,
    Info,
    Error,
}

impl LineKind {
    /// Color de la etiqueta.
    fn color(self, p: &Palette) -> Color32 {
        match self {
            Self::Rx => p.rx,
            Self::Tx => p.tx,
            Self::Info => p.text_dim,
            Self::Error => p.error,
        }
    }

    /// Color del texto de la línea.
    fn text_color(self, p: &Palette) -> Color32 {
        match self {
            Self::Rx => p.text,
            Self::Tx => p.tx,
            Self::Info => p.text_dim,
            Self::Error => p.error,
        }
    }

    /// Etiqueta en el log y al copiar.
    fn tag(self) -> &'static str {
        match self {
            Self::Rx => "RX",
            Self::Tx => "TX",
            Self::Info => "--",
            Self::Error => "!!",
        }
    }

    /// Etiqueta en pantalla (ancho fijo).
    fn chip(self) -> &'static str {
        match self {
            Self::Rx => "  RX  ",
            Self::Tx => "  TX  ",
            Self::Info => " INFO ",
            Self::Error => " ERR  ",
        }
    }
}

struct Line {
    time: DateTime<Local>,
    kind: LineKind,
    text: String,
}

#[derive(PartialEq, Eq)]
enum ConnState {
    Disconnected,
    Connecting(String),
    Connected(String),
}

/// Filtro de la lista de dispositivos.
struct DeviceFilter<'a> {
    /// Texto buscado en el nombre o en cualquier parte de la MAC.
    query: &'a str,
    min_rssi: Option<i16>,
    only_nus: bool,
}

impl DeviceFilter<'_> {
    fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || self.min_rssi.is_some() || self.only_nus
    }

    fn matches(&self, name: Option<&str>, address: &str, rssi: Option<i16>, has_nus: bool) -> bool {
        if self.only_nus && !has_nus {
            return false;
        }
        if let Some(min) = self.min_rssi
            && rssi.is_none_or(|r| r < min)
        {
            return false;
        }
        let query = self.query.trim();
        if query.is_empty() {
            return true;
        }
        let lower = query.to_lowercase();
        if name.is_some_and(|n| n.to_lowercase().contains(&lower)) {
            return true;
        }
        let mac = normalize_mac(query);
        !mac.is_empty() && normalize_mac(address).contains(&mac)
    }
}

/// MAC (o fragmento) sin separadores y en minúsculas: "A4:C1-38" → "a4c138".
fn normalize_mac(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub struct NusApp {
    cmd_tx: UnboundedSender<BleCommand>,
    evt_rx: Receiver<BleEvent>,
    config: Config,
    config_dirty: bool,

    scanning: bool,
    devices: HashMap<PeripheralId, DeviceInfo>,
    selected_device: Option<PeripheralId>,
    /// Dispositivo al que se está conectado (o conectando).
    connected_device: Option<PeripheralId>,
    search: String,
    conn: ConnState,
    mtu: u16,
    services: Vec<ServiceInfo>,
    selection: Selection,

    lines: Vec<Line>,
    /// Bytes RX de la línea en curso (aún sin '\n').
    rx_buf: Vec<u8>,
    /// La última línea es una RX incompleta que se va actualizando.
    rx_open: bool,

    input: String,
    history: Vec<String>,
    history_pos: Option<usize>,

    log: Option<SessionLog>,
    show_gatt: bool,
    show_macro_editor: bool,
    show_settings: bool,
}

impl NusApp {
    pub fn new(
        ctx: &egui::Context,
        cmd_tx: UnboundedSender<BleCommand>,
        evt_rx: Receiver<BleEvent>,
    ) -> Self {
        let (config, warning) = Config::load();
        theme::set_dark(ctx, config.dark_mode);
        let mut app = Self {
            cmd_tx,
            evt_rx,
            config,
            config_dirty: false,
            scanning: false,
            devices: HashMap::new(),
            selected_device: None,
            connected_device: None,
            search: String::new(),
            conn: ConnState::Disconnected,
            mtu: 23,
            services: Vec::new(),
            selection: Selection::default(),
            lines: Vec::new(),
            rx_buf: Vec::new(),
            rx_open: false,
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            log: None,
            show_gatt: true,
            show_macro_editor: false,
            show_settings: false,
        };
        if let Some(path) = Config::path() {
            if !path.exists() {
                app.save_config();
            }
            app.push(LineKind::Info, format!("Configuración: {}", path.display()));
        }
        if let Some(w) = warning {
            app.push(LineKind::Error, w);
        }
        app
    }

    // ---------------------------------------------------------------- lógica

    fn command(&mut self, cmd: BleCommand) {
        if self.cmd_tx.send(cmd).is_err() {
            self.push(LineKind::Error, "El worker BLE no está activo".into());
        }
    }

    fn push(&mut self, kind: LineKind, text: String) {
        self.close_rx_line();
        self.log_line(kind, &text);
        self.push_raw(kind, text);
    }

    fn push_raw(&mut self, kind: LineKind, text: String) {
        self.lines.push(Line {
            time: Local::now(),
            kind,
            text,
        });
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..MAX_LINES / 10);
        }
    }

    fn log_line(&mut self, kind: LineKind, text: &str) {
        if let Some(log) = &mut self.log
            && let Err(e) = log.write_line(kind.tag(), text)
        {
            self.log = None;
            self.push_raw(
                LineKind::Error,
                format!("Error escribiendo log, se detiene: {e}"),
            );
        }
    }

    /// Añade datos recibidos, partiendo por '\n' y mostrando la línea parcial.
    fn on_rx(&mut self, data: &[u8]) {
        if self.rx_open {
            self.lines.pop();
            self.rx_open = false;
        }
        self.rx_buf.extend_from_slice(data);
        while let Some(pos) = self.rx_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.rx_buf.drain(..=pos).collect();
            let text = decode_line(&line);
            self.log_line(LineKind::Rx, &text);
            self.push_raw(LineKind::Rx, text);
        }
        if !self.rx_buf.is_empty() {
            let text = decode_line(&self.rx_buf);
            self.push_raw(LineKind::Rx, text);
            self.rx_open = true;
        }
    }

    /// Cierra la línea RX parcial (p.ej. antes de mostrar un TX).
    fn close_rx_line(&mut self) {
        if self.rx_open {
            self.rx_open = false;
            let text = decode_line(&std::mem::take(&mut self.rx_buf));
            self.log_line(LineKind::Rx, &text);
        }
    }

    fn process_events(&mut self) {
        while let Ok(ev) = self.evt_rx.try_recv() {
            match ev {
                BleEvent::Info(s) => self.push(LineKind::Info, s),
                BleEvent::Error(s) => self.push(LineKind::Error, s),
                BleEvent::ScanState(s) => {
                    self.scanning = s;
                    self.push(
                        LineKind::Info,
                        if s {
                            "Escaneando…"
                        } else {
                            "Escaneo detenido"
                        }
                        .into(),
                    );
                }
                BleEvent::Device(d) => {
                    self.devices.insert(d.id.clone(), d);
                }
                BleEvent::Connecting(name) => {
                    self.push(LineKind::Info, format!("Conectando a {name}…"));
                    self.conn = ConnState::Connecting(name);
                }
                BleEvent::Connected { name, mtu } => {
                    self.push(LineKind::Info, format!("Conectado a {name} (MTU {mtu})"));
                    self.conn = ConnState::Connected(name);
                    self.mtu = mtu;
                }
                BleEvent::Services(s) => self.services = s,
                BleEvent::Selected(sel) => {
                    self.selection = sel;
                    let fmt =
                        |c: Option<CharRef>| c.map_or("—".to_string(), |c| uuid_label(c.uuid));
                    let text = format!(
                        "Escritura: {} · Notificación: {}",
                        fmt(sel.write),
                        fmt(sel.notify)
                    );
                    self.push(LineKind::Info, text);
                }
                BleEvent::Data(d) => self.on_rx(&d),
                BleEvent::Sent(d) => {
                    if self.config.echo {
                        let text = decode_line(&d);
                        self.push(LineKind::Tx, text);
                    }
                }
                BleEvent::Disconnected => {
                    if self.conn != ConnState::Disconnected {
                        self.push(LineKind::Info, "Desconectado".into());
                    }
                    self.conn = ConnState::Disconnected;
                    self.connected_device = None;
                    self.services.clear();
                    self.selection = Selection::default();
                }
            }
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.conn, ConnState::Connected(_))
    }

    fn send_text(&mut self, text: &str) {
        if !self.is_connected() {
            self.push(LineKind::Error, "No hay conexión".into());
            return;
        }
        let data = config::build_payload(text, self.config.line_ending);
        if data.is_empty() {
            return;
        }
        self.command(BleCommand::Send {
            data,
            chunk_size: self.config.chunk_size,
            with_response: self.config.write_with_response,
        });
    }

    fn submit_input(&mut self) {
        if !self.is_connected() {
            // Se conserva el texto para enviarlo cuando haya conexión.
            self.push(LineKind::Error, "No hay conexión".into());
            return;
        }
        let text = std::mem::take(&mut self.input);
        if !text.is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
        self.history_pos = None;
        self.send_text(&text);
    }

    fn run_macro(&mut self, m: &Macro) {
        match m.mode {
            MacroMode::Send => self.send_text(&m.text),
            MacroMode::Insert => self.input = m.text.clone(),
        }
    }

    fn connect_selected(&mut self) {
        if let Some(id) = self.selected_device.clone() {
            self.connected_device = Some(id.clone());
            self.command(BleCommand::Connect {
                id,
                prefer_write: self.config.default_write_uuid,
                prefer_notify: self.config.default_notify_uuid,
            });
        }
    }

    fn toggle_log(&mut self, path: Option<std::path::PathBuf>) {
        if self.log.is_some() && path.is_none() {
            self.close_rx_line();
            let p = self.log.take().map(|l| l.path().display().to_string());
            self.push(
                LineKind::Info,
                format!("Log cerrado: {}", p.unwrap_or_default()),
            );
            return;
        }
        let path = path.unwrap_or_else(SessionLog::default_path);
        match SessionLog::open(&path) {
            Ok(l) => {
                self.log = Some(l);
                self.push(
                    LineKind::Info,
                    format!("Guardando log en {}", path.display()),
                );
            }
            Err(e) => self.push(LineKind::Error, format!("No se pudo abrir el log: {e}")),
        }
    }

    fn choose_log_file(&mut self) {
        let default = SessionLog::default_path();
        let mut dlg = rfd::FileDialog::new()
            .add_filter("Log", &["log", "txt"])
            .set_file_name(
                default
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        if let Some(dir) = default.parent() {
            dlg = dlg.set_directory(dir);
        }
        if let Some(path) = dlg.save_file() {
            self.log = None;
            self.toggle_log(Some(path));
        }
    }

    fn clear_terminal(&mut self) {
        self.lines.clear();
        self.rx_buf.clear();
        self.rx_open = false;
    }

    fn set_dark(&mut self, ctx: &egui::Context, dark: bool) {
        self.config.dark_mode = dark;
        theme::set_dark(ctx, dark);
        self.config_dirty = true;
    }

    fn save_config(&mut self) {
        self.config_dirty = false;
        if let Err(e) = self.config.save() {
            self.push(
                LineKind::Error,
                format!("No se pudo guardar la configuración: {e:#}"),
            );
        }
    }

    // -------------------------------------------------------------------- UI

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::BLUETOOTH).size(22.0).color(p.accent));
            ui.label(RichText::new("NUS Client").size(16.0).strong());
            ui.add_space(12.0);
            self.status_pill(ui);

            theme::right_aligned(ui, 32.0, |ui| {
                let ctx = ui.ctx().clone();
                let (theme_icon, theme_tip) = if self.config.dark_mode {
                    (icon::SUN, "Cambiar a tema claro")
                } else {
                    (icon::MOON, "Cambiar a tema oscuro")
                };
                if theme::icon_button(ui, theme_icon, theme_tip).clicked() {
                    self.set_dark(&ctx, !self.config.dark_mode);
                }
                theme::icon_toggle(ui, &mut self.show_settings, icon::GEAR_SIX, "Ajustes");
                theme::icon_toggle(
                    ui,
                    &mut self.show_macro_editor,
                    icon::LIGHTNING,
                    "Editar macros",
                );
                let has_services = !self.services.is_empty();
                ui.add_enabled_ui(has_services, |ui| {
                    theme::icon_toggle(
                        ui,
                        &mut self.show_gatt,
                        icon::TREE_STRUCTURE,
                        "Panel de servicios",
                    )
                    .on_disabled_hover_text("Conéctate a un dispositivo para ver sus servicios");
                });
                ui.separator();

                match &self.conn {
                    ConnState::Disconnected => {
                        let enabled = self.selected_device.is_some();
                        let resp = theme::solid_button(
                            ui,
                            theme::label(icon::PLUGS, "Conectar"),
                            p.accent,
                            enabled,
                        )
                        .on_disabled_hover_text("Selecciona un dispositivo de la lista");
                        if resp.clicked() {
                            self.connect_selected();
                        }
                    }
                    ConnState::Connecting(_) | ConnState::Connected(_) => {
                        if theme::solid_button(
                            ui,
                            theme::label(icon::LINK_BREAK, "Desconectar"),
                            p.error,
                            true,
                        )
                        .clicked()
                        {
                            self.command(BleCommand::Disconnect);
                        }
                    }
                }
            });
        });
    }

    fn status_pill(&self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        let (color, text) = match &self.conn {
            ConnState::Disconnected => (p.text_dim, "Desconectado".to_string()),
            ConnState::Connecting(n) => (p.warn, format!("Conectando a {n}…")),
            ConnState::Connected(n) => (p.success, format!("{n}  ·  MTU {}", self.mtu)),
        };
        egui::Frame::new()
            .fill(color.gamma_multiply(0.14))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.horizontal(|ui| {
                    if matches!(self.conn, ConnState::Connecting(_)) {
                        ui.add(egui::Spinner::new().size(12.0).color(color));
                    } else {
                        ui.label(theme::filled(icon::CIRCLE).size(10.0).color(color));
                    }
                    ui.label(RichText::new(text).color(color).strong());
                });
            });
        if let Some(log) = &self.log {
            theme::pill(ui, theme::label(icon::RECORD, "REC"), p.error)
                .on_hover_text(format!("Guardando log en {}", log.path().display()));
        }
    }

    fn devices_panel(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);

        // Cabecera
        ui.horizontal(|ui| {
            theme::section_title(ui, icon::BROADCAST, "Dispositivos");
            if !self.devices.is_empty() {
                theme::pill(ui, self.devices.len().to_string(), p.text_dim);
            }
            theme::right_aligned(ui, 30.0, |ui| {
                if theme::icon_button(ui, icon::BROOM, "Vaciar la lista").clicked() {
                    self.devices.clear();
                }
            });
        });

        // Escanear / detener
        ui.horizontal(|ui| {
            let size = [ui.available_width(), 34.0];
            if self.scanning {
                ui.add(egui::Spinner::new().size(18.0).color(p.accent));
                let size = [ui.available_width(), 34.0];
                if ui
                    .add_sized(
                        size,
                        egui::Button::new(theme::label(icon::STOP, "Detener escaneo")),
                    )
                    .clicked()
                {
                    self.command(BleCommand::StopScan);
                }
            } else if ui
                .add_sized(
                    size,
                    egui::Button::new(
                        RichText::new(theme::label(icon::MAGNIFYING_GLASS, "Escanear"))
                            .color(p.on_accent)
                            .strong(),
                    )
                    .fill(p.accent),
                )
                .clicked()
            {
                self.command(BleCommand::StartScan);
            }
        });
        ui.add_space(2.0);

        // Filtros
        self.device_filters(ui);
        ui.add_space(2.0);

        let filter = DeviceFilter {
            query: &self.search,
            min_rssi: self.config.min_rssi,
            only_nus: self.config.only_nus,
        };
        let mut list: Vec<&DeviceInfo> = self
            .devices
            .values()
            .filter(|d| filter.matches(d.name.as_deref(), &d.address, d.rssi, d.has_nus))
            .collect();
        list.sort_by(|a, b| {
            b.rssi
                .unwrap_or(i16::MIN)
                .cmp(&a.rssi.unwrap_or(i16::MIN))
                .then_with(|| a.address.cmp(&b.address))
        });

        let hidden = self.devices.len() - list.len();
        let mut clear_filters = false;
        if filter.is_active() && hidden > 0 {
            ui.horizontal(|ui| {
                theme::caption(
                    ui,
                    format!("Mostrando {} de {}", list.len(), self.devices.len()),
                );
                theme::right_aligned(ui, 22.0, |ui| {
                    let btn = egui::Button::new(
                        RichText::new(theme::label(icon::FUNNEL_X, "Quitar filtros"))
                            .small()
                            .color(p.accent),
                    )
                    .frame_when_inactive(false);
                    clear_filters = ui.add(btn).clicked();
                });
            });
        }

        let mut clicked = None;
        let mut double = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if list.is_empty() {
                    if !self.devices.is_empty() {
                        theme::empty_state(
                            ui,
                            icon::FUNNEL,
                            "Sin coincidencias",
                            "Ningún dispositivo coincide con los filtros.",
                        );
                    } else if self.scanning {
                        theme::empty_state(
                            ui,
                            icon::BROADCAST,
                            "Buscando…",
                            "Los dispositivos cercanos aparecerán aquí.",
                        );
                    } else {
                        theme::empty_state(
                            ui,
                            icon::BLUETOOTH,
                            "Sin dispositivos",
                            "Pulsa «Escanear» para buscar dispositivos BLE.",
                        );
                    }
                }
                for d in list {
                    let selected = self.selected_device.as_ref() == Some(&d.id);
                    let active = self.connected_device.as_ref() == Some(&d.id);
                    let resp = device_card(ui, d, selected, active);
                    if resp.clicked() {
                        clicked = Some(d.id.clone());
                    }
                    if resp.double_clicked() {
                        double = Some(d.id.clone());
                    }
                }
            });

        if clear_filters {
            self.search.clear();
            self.config.min_rssi = None;
            self.config.only_nus = false;
            self.config_dirty = true;
        }
        if let Some(id) = clicked {
            self.selected_device = Some(id);
        }
        if let Some(id) = double {
            self.selected_device = Some(id);
            if self.conn == ConnState::Disconnected {
                self.connect_selected();
            }
        }
    }

    fn device_filters(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);

        // Búsqueda por nombre o MAC
        egui::Frame::new()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_dim));
                    let clear_w = if self.search.is_empty() { 0.0 } else { 26.0 };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Buscar por nombre o MAC…")
                            .frame(egui::Frame::NONE)
                            .margin(Margin::symmetric(2, 6))
                            .desired_width(ui.available_width() - clear_w),
                    );
                    resp.on_hover_text(
                        "Coincide con el nombre o con cualquier parte de la MAC\n\
                         (sin importar mayúsculas ni separadores: «a4c1», «C1:38»…)",
                    );
                    if !self.search.is_empty()
                        && ui
                            .add(
                                egui::Button::new(RichText::new(icon::X).color(p.text_dim))
                                    .frame_when_inactive(false)
                                    .small(),
                            )
                            .on_hover_text("Borrar búsqueda")
                            .clicked()
                    {
                        self.search.clear();
                    }
                });
            });

        // Chips: sólo NUS / RSSI mínimo
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if theme::chip_toggle(ui, &mut self.config.only_nus, icon::FUNNEL, "Sólo NUS")
                .on_hover_text("Mostrar sólo dispositivos que anuncian Nordic UART Service")
                .changed()
            {
                self.config_dirty = true;
            }
            let mut rssi_on = self.config.min_rssi.is_some();
            let rssi_text = match self.config.min_rssi {
                Some(v) => format!("RSSI ≥ {v} dBm"),
                None => "RSSI mínimo".to_string(),
            };
            if theme::chip_toggle(ui, &mut rssi_on, icon::CELL_SIGNAL_HIGH, &rssi_text)
                .on_hover_text("Ocultar dispositivos con señal más débil")
                .changed()
            {
                self.config.min_rssi = rssi_on.then_some(DEFAULT_MIN_RSSI);
                self.config_dirty = true;
            }
        });

        if let Some(v) = &mut self.config.min_rssi {
            ui.horizontal(|ui| {
                theme::caption(ui, "−100").on_hover_text("Señal débil");
                // Deja sitio a la etiqueta final para no ensanchar el panel.
                ui.spacing_mut().slider_width = (ui.available_width() - 44.0).max(60.0);
                let resp = ui
                    .add(
                        egui::Slider::new(v, -100..=-30)
                            .show_value(false)
                            .step_by(1.0),
                    )
                    .on_hover_text("Arrastra para cambiar el RSSI mínimo");
                if resp.changed() {
                    self.config_dirty = true;
                }
                theme::caption(ui, "−30").on_hover_text("Señal fuerte");
            });
        }
    }

    fn gatt_panel(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        ui.horizontal(|ui| {
            theme::section_title(ui, icon::TREE_STRUCTURE, "Servicios");
            theme::right_aligned(ui, 30.0, |ui| {
                if theme::icon_button(ui, icon::X, "Ocultar panel").clicked() {
                    self.show_gatt = false;
                }
            });
        });

        theme::card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut sel = self.selection;
            char_combo(
                ui,
                "write_combo",
                icon::PENCIL_SIMPLE,
                "Escritura (TX)",
                &self.services,
                &mut sel.write,
                true,
            );
            ui.add_space(2.0);
            char_combo(
                ui,
                "notify_combo",
                icon::BELL,
                "Notificación (RX)",
                &self.services,
                &mut sel.notify,
                false,
            );
            if sel != self.selection {
                self.command(BleCommand::Select(sel));
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(theme::label(icon::ARROW_COUNTER_CLOCKWISE, "Usar NUS"))
                    .on_hover_text("Seleccionar las características NUS estándar")
                    .clicked()
                {
                    let sel = Selection {
                        write: Some(CharRef {
                            service: NUS_SERVICE,
                            uuid: NUS_RX_WRITE,
                        }),
                        notify: Some(CharRef {
                            service: NUS_SERVICE,
                            uuid: NUS_TX_NOTIFY,
                        }),
                    };
                    self.command(BleCommand::Select(sel));
                }
                if ui
                    .button(theme::label(icon::FLOPPY_DISK, "Predeterminar"))
                    .on_hover_text("Usar estas características al conectar")
                    .clicked()
                {
                    if let Some(w) = self.selection.write {
                        self.config.default_write_uuid = w.uuid;
                    }
                    if let Some(n) = self.selection.notify {
                        self.config.default_notify_uuid = n.uuid;
                    }
                    self.config_dirty = true;
                }
            });
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for s in &self.services {
                    theme::card_frame(ui)
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let title = uuid_name(s.uuid)
                                .map_or_else(|| "Servicio".to_string(), str::to_owned);
                            egui::CollapsingHeader::new(RichText::new(title).strong())
                                .id_salt(s.uuid)
                                .default_open(true)
                                .show(ui, |ui| {
                                    theme::caption(ui, s.uuid.to_string());
                                    for c in &s.chars {
                                        ui.add_space(2.0);
                                        self.char_row(ui, p, s.uuid, c.uuid, c.props);
                                    }
                                });
                        });
                }
            });
    }

    fn char_row(
        &self,
        ui: &mut egui::Ui,
        p: &Palette,
        service: Uuid,
        uuid: Uuid,
        props: CharPropFlags,
    ) {
        let r = CharRef { service, uuid };
        egui::Frame::new()
            .fill(p.raised.gamma_multiply(0.6))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                ui.horizontal_wrapped(|ui| {
                    let title =
                        uuid_name(uuid).map_or_else(|| "Característica".to_string(), str::to_owned);
                    ui.label(RichText::new(title).strong());
                    if self.selection.write == Some(r) {
                        theme::pill(ui, theme::label(icon::PENCIL_SIMPLE, "TX"), p.tx)
                            .on_hover_text("Característica de escritura activa");
                    }
                    if self.selection.notify == Some(r) {
                        theme::pill(ui, theme::label(icon::BELL, "RX"), p.rx)
                            .on_hover_text("Característica de notificación activa");
                    }
                });
                ui.label(
                    RichText::new(uuid.to_string())
                        .monospace()
                        .size(11.0)
                        .color(p.text_dim),
                );
                ui.horizontal_wrapped(|ui| {
                    for name in props_names(props) {
                        theme::pill(ui, name, p.text_dim);
                    }
                });
            });
    }

    fn input_panel(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        let connected = self.is_connected();

        // Macros
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icon::LIGHTNING).color(p.text_dim))
                .on_hover_text("Macros");
            let mut run = None;
            for (i, m) in self.config.macros.iter().enumerate() {
                let name = if m.name.is_empty() { &m.text } else { &m.name };
                let (ic, hint) = match m.mode {
                    MacroMode::Send => (icon::PAPER_PLANE_TILT, format!("Enviar: {}", m.text)),
                    MacroMode::Insert => (
                        icon::CURSOR_TEXT,
                        format!("Insertar en la entrada: {}", m.text),
                    ),
                };
                let btn =
                    egui::Button::new(theme::label(ic, name)).corner_radius(CornerRadius::same(14));
                if ui.add(btn).on_hover_text(hint).clicked() {
                    run = Some(i);
                }
            }
            if theme::icon_button(ui, icon::PENCIL_SIMPLE, "Editar macros").clicked() {
                self.show_macro_editor = true;
            }
            if let Some(i) = run {
                let m = self.config.macros[i].clone();
                self.run_macro(&m);
            }
        });
        ui.add_space(2.0);

        // Entrada
        theme::right_aligned(ui, 36.0, |ui| {
            let send_clicked = ui
                .add_enabled(
                    connected,
                    egui::Button::new(
                        RichText::new(theme::label(icon::PAPER_PLANE_RIGHT, "Enviar"))
                            .color(p.on_accent)
                            .strong(),
                    )
                    .fill(p.accent)
                    .min_size(vec2(100.0, 34.0)),
                )
                .on_disabled_hover_text("Conéctate a un dispositivo para enviar")
                .clicked();

            let mut le = self.config.line_ending;
            egui::ComboBox::from_id_salt("line_ending")
                .selected_text(theme::label(icon::KEY_RETURN, le.label()))
                .width(110.0)
                .height(34.0)
                .show_ui(ui, |ui| {
                    for opt in LineEnding::ALL {
                        ui.selectable_value(&mut le, opt, opt.label());
                    }
                })
                .response
                .on_hover_text("Fin de línea añadido a cada mensaje");
            if le != self.config.line_ending {
                self.config.line_ending = le;
                self.config_dirty = true;
            }

            let hint = if connected {
                "Escribe un mensaje y pulsa Enter  (↑/↓ historial)"
            } else {
                "Conéctate a un dispositivo para enviar mensajes"
            };
            let edit = egui::TextEdit::singleline(&mut self.input)
                .hint_text(hint)
                .font(egui::TextStyle::Monospace)
                .margin(Margin::symmetric(10, 8))
                .desired_width(ui.available_width());
            let resp = ui.add(edit);

            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if resp.has_focus() {
                let (up, down) =
                    ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
                if up && !self.history.is_empty() {
                    let pos = self
                        .history_pos
                        .map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                    self.history_pos = Some(pos);
                    self.input = self.history[pos].clone();
                } else if down {
                    match self.history_pos {
                        Some(p) if p + 1 < self.history.len() => {
                            self.history_pos = Some(p + 1);
                            self.input = self.history[p + 1].clone();
                        }
                        Some(_) => {
                            self.history_pos = None;
                            self.input.clear();
                        }
                        None => {}
                    }
                }
            }
            if enter || send_clicked {
                self.submit_input();
                resp.request_focus();
            }
        });
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        theme::card_frame(ui)
            .inner_margin(Margin::ZERO)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Cabecera
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        ui.horizontal(|ui| {
                            // Las acciones ocupan ~300 px; el título sólo si sobra sitio.
                            let width = ui.available_width();
                            if width > 360.0 {
                                ui.label(
                                    RichText::new(icon::TERMINAL_WINDOW)
                                        .size(18.0)
                                        .color(p.accent),
                                );
                                ui.label(RichText::new("Terminal").strong());
                            }
                            if width > 470.0 && !self.lines.is_empty() {
                                theme::pill(ui, format!("{} líneas", self.lines.len()), p.text_dim);
                            }
                            theme::right_aligned(ui, 30.0, |ui| self.terminal_actions(ui, p));
                        });
                    });
                ui.add(egui::Separator::default().spacing(0.0));

                // Cuerpo
                egui::Frame::new()
                    .fill(p.field)
                    .corner_radius(CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 9,
                        se: 9,
                    })
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        if self.lines.is_empty() {
                            theme::empty_state(
                                ui,
                                icon::TERMINAL_WINDOW,
                                "Sin actividad",
                                "Los datos enviados y recibidos aparecerán aquí.",
                            );
                        } else {
                            self.terminal_lines(ui, p);
                        }
                    });
            });
    }

    fn terminal_actions(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if theme::icon_button(ui, icon::TRASH, "Limpiar terminal").clicked() {
            self.clear_terminal();
        }
        if theme::icon_button(ui, icon::COPY, "Copiar todo al portapapeles").clicked() {
            let text = self.terminal_text();
            ui.ctx().copy_text(text);
        }
        ui.separator();
        if theme::icon_button(ui, icon::FOLDER_OPEN, "Guardar log en un archivo…").clicked() {
            self.choose_log_file();
        }
        let logging = self.log.is_some();
        let rec = if logging {
            theme::filled(icon::RECORD).size(17.0).color(p.error)
        } else {
            RichText::new(icon::RECORD).size(17.0)
        };
        let tip = if logging {
            "Detener el log de sesión"
        } else {
            "Guardar la sesión en un archivo (ruta automática)"
        };
        if ui
            .add(egui::Button::selectable(logging, rec).min_size(vec2(30.0, 30.0)))
            .on_hover_text(tip)
            .clicked()
        {
            self.toggle_log(None);
        }
        ui.separator();
        let mut changed = false;
        changed |= theme::icon_toggle(
            ui,
            &mut self.config.echo,
            icon::ARROWS_LEFT_RIGHT,
            "Eco local: mostrar lo enviado",
        )
        .changed();
        changed |= theme::icon_toggle(ui, &mut self.config.timestamps, icon::CLOCK, "Mostrar hora")
            .changed();
        self.config_dirty |= changed;
    }

    fn terminal_lines(&self, ui: &mut egui::Ui, p: &Palette) {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
        let ts = self.config.timestamps;
        egui::ScrollArea::both()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show_rows(ui, row_h, self.lines.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 3.0;
                for line in &self.lines[range] {
                    let mut job = LayoutJob::default();
                    if ts {
                        job.append(
                            &line.time.format("%H:%M:%S%.3f").to_string(),
                            0.0,
                            TextFormat::simple(font.clone(), p.text_dim.gamma_multiply(0.8)),
                        );
                    }
                    let tag = line.kind.color(p);
                    job.append(
                        line.kind.chip(),
                        if ts { 10.0 } else { 0.0 },
                        TextFormat {
                            font_id: font.clone(),
                            color: tag,
                            background: tag.gamma_multiply(0.16),
                            ..Default::default()
                        },
                    );
                    job.append(
                        &line.text,
                        10.0,
                        TextFormat::simple(font.clone(), line.kind.text_color(p)),
                    );
                    ui.add(egui::Label::new(job).extend());
                }
            });
    }

    fn terminal_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                let ts = if self.config.timestamps {
                    l.time.format("%H:%M:%S%.3f ").to_string()
                } else {
                    String::new()
                };
                format!("{ts}{} {}\n", l.kind.tag(), l.text)
            })
            .collect()
    }

    fn macro_editor(&mut self, ctx: &egui::Context) {
        let mut open = self.show_macro_editor;
        egui::Window::new(theme::label(icon::LIGHTNING, "Macros"))
            .id(egui::Id::new("macro_editor"))
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_width(600.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = Palette::of(ui);
                theme::caption(
                    ui,
                    "«Enviar» manda el texto al pulsar el botón; «Insertar» lo copia a la \
                     entrada para completarlo. Los cambios se guardan automáticamente.",
                );
                ui.add_space(4.0);
                let mut action: Option<(usize, i32)> = None; // (índice, -1 subir / 1 bajar / 0 borrar)
                let len = self.config.macros.len();
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        if len == 0 {
                            theme::empty_state(
                                ui,
                                icon::LIGHTNING,
                                "Sin macros",
                                "Añade una para enviar comandos con un clic.",
                            );
                            return;
                        }
                        egui::Grid::new("macro_grid")
                            .striped(true)
                            .num_columns(4)
                            .spacing(vec2(8.0, 6.0))
                            .show(ui, |ui| {
                                for h in ["Nombre", "Texto", "Modo", ""] {
                                    ui.label(RichText::new(h).small().strong().color(p.text_dim));
                                }
                                ui.end_row();
                                for (i, m) in self.config.macros.iter_mut().enumerate() {
                                    let mut changed = false;
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut m.name)
                                                .hint_text("Nombre")
                                                .min_size(vec2(130.0, 0.0))
                                                .desired_width(130.0),
                                        )
                                        .changed();
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut m.text)
                                                .hint_text("Texto a enviar")
                                                .font(egui::TextStyle::Monospace)
                                                .min_size(vec2(220.0, 0.0))
                                                .desired_width(220.0),
                                        )
                                        .changed();
                                    let mode_label = |mode| match mode {
                                        MacroMode::Send => {
                                            theme::label(icon::PAPER_PLANE_TILT, "Enviar")
                                        }
                                        MacroMode::Insert => {
                                            theme::label(icon::CURSOR_TEXT, "Insertar")
                                        }
                                    };
                                    egui::ComboBox::from_id_salt(("macro_mode", i))
                                        .selected_text(mode_label(m.mode))
                                        .width(110.0)
                                        .show_ui(ui, |ui| {
                                            for mode in [MacroMode::Send, MacroMode::Insert] {
                                                changed |= ui
                                                    .selectable_value(
                                                        &mut m.mode,
                                                        mode,
                                                        mode_label(mode),
                                                    )
                                                    .changed();
                                            }
                                        });
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        ui.add_enabled_ui(i > 0, |ui| {
                                            if theme::icon_button(ui, icon::ARROW_UP, "Subir")
                                                .clicked()
                                            {
                                                action = Some((i, -1));
                                            }
                                        });
                                        ui.add_enabled_ui(i + 1 < len, |ui| {
                                            if theme::icon_button(ui, icon::ARROW_DOWN, "Bajar")
                                                .clicked()
                                            {
                                                action = Some((i, 1));
                                            }
                                        });
                                        let del = egui::Button::new(
                                            RichText::new(icon::TRASH).size(17.0).color(p.error),
                                        )
                                        .frame_when_inactive(false)
                                        .min_size(vec2(30.0, 30.0));
                                        if ui.add(del).on_hover_text("Eliminar").clicked() {
                                            action = Some((i, 0));
                                        }
                                    });
                                    ui.end_row();
                                    self.config_dirty |= changed;
                                }
                            });
                    });
                if let Some((i, a)) = action {
                    match a {
                        0 => {
                            self.config.macros.remove(i);
                        }
                        -1 => self.config.macros.swap(i, i - 1),
                        _ => self.config.macros.swap(i, i + 1),
                    }
                    self.config_dirty = true;
                }
                ui.add_space(6.0);
                if theme::accent_button(ui, theme::label(icon::PLUS, "Añadir macro")).clicked() {
                    self.config.macros.push(Macro {
                        name: format!("M{}", self.config.macros.len() + 1),
                        ..Default::default()
                    });
                    self.config_dirty = true;
                }
            });
        self.show_macro_editor = open;
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new(theme::label(icon::GEAR_SIX, "Ajustes"))
            .id(egui::Id::new("settings"))
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .min_width(520.0)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let p = Palette::of(ui);
                let mut changed = false;
                let heading = |ui: &mut egui::Ui, ic: &str, text: &str| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ic).color(p.accent));
                        ui.label(RichText::new(text).strong());
                    });
                };

                heading(ui, icon::PALETTE, "Apariencia");
                ui.horizontal(|ui| {
                    let dark = self.config.dark_mode;
                    if ui
                        .selectable_label(dark, theme::label(icon::MOON, "Oscuro"))
                        .clicked()
                        && !dark
                    {
                        self.set_dark(ctx, true);
                    }
                    if ui
                        .selectable_label(!dark, theme::label(icon::SUN, "Claro"))
                        .clicked()
                        && dark
                    {
                        self.set_dark(ctx, false);
                    }
                });
                ui.add_space(6.0);

                heading(ui, icon::PAPER_PLANE_RIGHT, "Escritura");
                egui::Grid::new("settings_write")
                    .num_columns(2)
                    .spacing(vec2(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.label("Tamaño de trozo");
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut self.config.chunk_size)
                                        .range(0..=512)
                                        .suffix(" B"),
                                )
                                .changed();
                            theme::caption(
                                ui,
                                format!(
                                    "0 = auto (ahora {} B)",
                                    config::effective_chunk_size(self.config.chunk_size, self.mtu)
                                ),
                            );
                        });
                        ui.end_row();

                        ui.label("Con respuesta");
                        changed |= ui
                            .checkbox(
                                &mut self.config.write_with_response,
                                "Confirmar cada escritura",
                            )
                            .on_hover_text(
                                "Más lento pero confirmado. Si la característica sólo admite \
                                 un modo, se usa ese.",
                            )
                            .changed();
                        ui.end_row();
                    });
                ui.add_space(6.0);

                heading(ui, icon::TREE_STRUCTURE, "Características predeterminadas");
                egui::Grid::new("settings_uuids")
                    .num_columns(2)
                    .spacing(vec2(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.label(theme::label(icon::PENCIL_SIMPLE, "Escritura"));
                        changed |= uuid_edit(ui, &mut self.config.default_write_uuid);
                        ui.end_row();

                        ui.label(theme::label(icon::BELL, "Notificación"));
                        changed |= uuid_edit(ui, &mut self.config.default_notify_uuid);
                        ui.end_row();
                    });
                ui.add_space(4.0);
                if ui
                    .button(theme::label(
                        icon::ARROW_COUNTER_CLOCKWISE,
                        "Restaurar UUIDs NUS",
                    ))
                    .clicked()
                {
                    self.config.default_write_uuid = NUS_RX_WRITE;
                    self.config.default_notify_uuid = NUS_TX_NOTIFY;
                    changed = true;
                }
                self.config_dirty |= changed;
            });
        self.show_settings = open;
    }
}

impl eframe::App for NusApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Palette::of(ui);

        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("input")
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| self.input_panel(ui));
        egui::Panel::left("devices")
            .resizable(true)
            .show_separator_line(false)
            .default_size(290.0)
            .size_range(210.0..=460.0)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(12)))
            .show(ui, |ui| self.devices_panel(ui));
        if self.show_gatt && !self.services.is_empty() {
            egui::Panel::right("gatt")
                .resizable(true)
                .show_separator_line(false)
                .default_size(290.0)
                .size_range(230.0..=520.0)
                .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(12)))
                .show(ui, |ui| self.gatt_panel(ui));
        }
        egui::CentralPanel::no_frame()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(2, 12)),
            )
            .show(ui, |ui| self.terminal(ui));

        if self.show_macro_editor {
            self.macro_editor(&ctx);
        }
        if self.show_settings {
            self.settings_window(&ctx);
        }

        if self.config_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.save_config();
        }
    }

    fn on_exit(&mut self) {
        if self.config_dirty {
            let _ = self.config.save();
        }
    }
}

// ------------------------------------------------------------------ helpers

/// Tarjeta clicable de un dispositivo de la lista.
fn device_card(ui: &mut egui::Ui, d: &DeviceInfo, selected: bool, active: bool) -> egui::Response {
    let p = Palette::of(ui);
    let id = ui.make_persistent_id(("device", &d.id));
    let mut prepared = egui::Frame::new()
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 8))
        .stroke(Stroke::new(1.0, p.border))
        .begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        theme::right_aligned(ui, 38.0, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (sig, color) = theme::signal(d.rssi, p);
            if let Some(r) = d.rssi {
                ui.label(RichText::new(format!("{r} dBm")).small().color(p.text_dim));
            }
            ui.label(RichText::new(sig).size(18.0).color(color));
            if d.has_nus {
                theme::pill(ui, "NUS", p.accent);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (ic, ic_color) = if active {
                    (icon::PLUGS_CONNECTED, p.success)
                } else if selected {
                    (icon::BLUETOOTH, p.accent)
                } else {
                    (icon::BLUETOOTH, p.text_dim)
                };
                ui.label(RichText::new(ic).size(20.0).color(ic_color));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let name = match d.name.as_deref() {
                        Some(n) if !n.trim().is_empty() => RichText::new(n).strong().color(p.text),
                        _ => RichText::new("Sin nombre").italics().color(p.text_dim),
                    };
                    ui.add(egui::Label::new(name).truncate().selectable(false));
                    ui.add(
                        egui::Label::new(
                            RichText::new(&d.address)
                                .monospace()
                                .size(11.5)
                                .color(p.text_dim),
                        )
                        .truncate()
                        .selectable(false),
                    );
                });
            });
        });
    }
    let rect = prepared.allocate_space(ui).rect;
    let resp = ui.interact(rect, id, Sense::click());
    let (fill, stroke) = if selected {
        (mix(p.surface, p.accent, 0.16), p.accent)
    } else if resp.hovered() {
        (p.raised, mix(p.border, p.accent, 0.4))
    } else {
        (p.surface, p.border)
    };
    prepared.frame.fill = fill;
    prepared.frame.stroke = Stroke::new(1.0, stroke);
    prepared.paint(ui);
    resp.on_hover_text("Doble clic para conectar")
}

/// Decodifica UTF-8 (con pérdida) y quita el fin de línea.
fn decode_line(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim_end_matches(['\r', '\n']).replace('\r', "\\r")
}

/// Nombre de un UUID conocido.
fn uuid_name(u: Uuid) -> Option<&'static str> {
    match u {
        NUS_SERVICE => Some("NUS"),
        NUS_RX_WRITE => Some("NUS RX"),
        NUS_TX_NOTIFY => Some("NUS TX"),
        _ => short_uuid_name(u),
    }
}

/// UUID con nombre si es conocido.
fn uuid_label(u: Uuid) -> String {
    match uuid_name(u) {
        Some(n) => format!("{n} ({u})"),
        None => u.to_string(),
    }
}

/// Nombres de algunos UUID SIG de 16 bits habituales.
fn short_uuid_name(u: Uuid) -> Option<&'static str> {
    const BASE_LOW: u128 = 0x0000_0000_0000_1000_8000_0080_5F9B_34FB;
    let v = u.as_u128();
    if v & !(0xFFFF_u128 << 96) != BASE_LOW {
        return None;
    }
    Some(match (v >> 96) as u16 {
        0x1800 => "Generic Access",
        0x1801 => "Generic Attribute",
        0x180A => "Device Information",
        0x180F => "Battery",
        0x2A00 => "Device Name",
        0x2A01 => "Appearance",
        0x2A05 => "Service Changed",
        0x2A19 => "Battery Level",
        0x2A24 => "Model Number",
        0x2A25 => "Serial Number",
        0x2A26 => "Firmware Rev",
        0x2A29 => "Manufacturer",
        _ => return None,
    })
}

fn props_names(p: CharPropFlags) -> Vec<&'static str> {
    let names = [
        (CharPropFlags::READ, "Read"),
        (CharPropFlags::WRITE, "Write"),
        (CharPropFlags::WRITE_WITHOUT_RESPONSE, "WriteNoResp"),
        (CharPropFlags::NOTIFY, "Notify"),
        (CharPropFlags::INDICATE, "Indicate"),
        (CharPropFlags::BROADCAST, "Broadcast"),
        (CharPropFlags::AUTHENTICATED_SIGNED_WRITES, "SignedWrite"),
    ];
    names
        .iter()
        .filter(|(f, _)| p.contains(*f))
        .map(|(_, n)| *n)
        .collect()
}

fn char_combo(
    ui: &mut egui::Ui,
    id: &str,
    icon: &str,
    label: &str,
    services: &[ServiceInfo],
    value: &mut Option<CharRef>,
    write: bool,
) {
    let p = Palette::of(ui);
    ui.label(
        RichText::new(theme::label(icon, label))
            .small()
            .strong()
            .color(p.text_dim),
    );
    let selected = value.map_or("Ninguna".to_string(), |c| {
        uuid_name(c.uuid).map_or_else(|| c.uuid.to_string(), str::to_owned)
    });
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(ui.available_width())
        .truncate()
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "Ninguna");
            for s in services {
                let chars: Vec<_> = s
                    .chars
                    .iter()
                    .filter(|c| if write { c.can_write() } else { c.can_notify() })
                    .collect();
                if chars.is_empty() {
                    continue;
                }
                ui.label(RichText::new(uuid_label(s.uuid)).small().color(p.text_dim));
                for c in chars {
                    let r = CharRef {
                        service: s.uuid,
                        uuid: c.uuid,
                    };
                    ui.selectable_value(value, Some(r), format!("   {}", uuid_label(c.uuid)));
                }
            }
        });
}

/// Campo de texto para un UUID; sólo aplica el valor si es válido.
fn uuid_edit(ui: &mut egui::Ui, value: &mut Uuid) -> bool {
    let id = ui.next_auto_id();
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| value.to_string());
    let valid = Uuid::parse_str(text.trim()).is_ok();
    let mut edit = egui::TextEdit::singleline(&mut text)
        .font(egui::TextStyle::Monospace)
        .min_size(vec2(330.0, 0.0))
        .desired_width(330.0);
    if !valid {
        edit = edit.text_color(Palette::of(ui).error);
    }
    let resp = ui.add(edit);
    let mut changed = false;
    if resp.changed()
        && let Ok(u) = Uuid::parse_str(text.trim())
    {
        *value = u;
        changed = true;
    }
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "A4:C1:38:E2:5F:0B";

    fn filter(query: &str) -> DeviceFilter<'_> {
        DeviceFilter {
            query,
            min_rssi: None,
            only_nus: false,
        }
    }

    #[test]
    fn mac_normalization() {
        assert_eq!(normalize_mac("A4:C1-38 e2"), "a4c138e2");
        assert_eq!(normalize_mac(" : - "), "");
    }

    #[test]
    fn empty_filter_matches_everything() {
        let f = filter("  ");
        assert!(!f.is_active());
        assert!(f.matches(None, ADDR, None, false));
    }

    #[test]
    fn matches_name_case_insensitive() {
        let f = filter("nordic");
        assert!(f.matches(Some("My Nordic UART"), ADDR, Some(-50), false));
        assert!(!f.matches(Some("Sensor"), ADDR, Some(-50), false));
        assert!(!f.matches(None, ADDR, Some(-50), false));
    }

    #[test]
    fn matches_any_part_of_mac() {
        for q in [
            "a4c1",
            "A4:C1",
            "c1:38",
            "38E2",
            "e2-5f",
            "5f0b",
            "0B",
            "a4:c1:38:e2:5f:0b",
        ] {
            assert!(filter(q).matches(None, ADDR, None, false), "{q}");
        }
        for q in ["a4c2", "c1:39", "zz"] {
            assert!(!filter(q).matches(None, ADDR, None, false), "{q}");
        }
    }

    #[test]
    fn min_rssi_threshold() {
        let f = DeviceFilter {
            query: "",
            min_rssi: Some(-70),
            only_nus: false,
        };
        assert!(f.is_active());
        assert!(f.matches(None, ADDR, Some(-70), false));
        assert!(f.matches(None, ADDR, Some(-40), false));
        assert!(!f.matches(None, ADDR, Some(-71), false));
        assert!(!f.matches(None, ADDR, None, false));
    }

    #[test]
    fn filters_combine() {
        let f = DeviceFilter {
            query: "38e2",
            min_rssi: Some(-80),
            only_nus: true,
        };
        assert!(f.matches(Some("x"), ADDR, Some(-60), true));
        assert!(!f.matches(Some("x"), ADDR, Some(-60), false));
        assert!(!f.matches(Some("x"), ADDR, Some(-90), true));
        assert!(!f.matches(Some("x"), "00:11:22:33:44:55", Some(-60), true));
    }
}
