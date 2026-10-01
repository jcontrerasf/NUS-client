//! Interfaz gráfica egui.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use btleplug::api::CharPropFlags;
use btleplug::platform::PeripheralId;
use chrono::{DateTime, Local};
use eframe::egui::{self, Color32, Key, RichText};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::config::{
    self, Config, LineEnding, Macro, MacroMode, NUS_RX_WRITE, NUS_SERVICE, NUS_TX_NOTIFY,
};
use crate::log::SessionLog;
use crate::messages::{BleCommand, BleEvent, CharRef, DeviceInfo, Selection, ServiceInfo};

const MAX_LINES: usize = 20_000;
const MAX_HISTORY: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Rx,
    Tx,
    Info,
    Error,
}

impl LineKind {
    fn color(self, dark: bool) -> Color32 {
        match (self, dark) {
            (Self::Rx, true) => Color32::from_rgb(120, 220, 140),
            (Self::Rx, false) => Color32::from_rgb(0, 120, 40),
            (Self::Tx, true) => Color32::from_rgb(120, 180, 255),
            (Self::Tx, false) => Color32::from_rgb(20, 80, 200),
            (Self::Info, true) => Color32::from_gray(160),
            (Self::Info, false) => Color32::from_gray(100),
            (Self::Error, _) => Color32::from_rgb(230, 70, 70),
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Rx => "RX",
            Self::Tx => "TX",
            Self::Info => "--",
            Self::Error => "!!",
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

pub struct NusApp {
    cmd_tx: UnboundedSender<BleCommand>,
    evt_rx: Receiver<BleEvent>,
    config: Config,
    config_dirty: bool,

    scanning: bool,
    devices: HashMap<PeripheralId, DeviceInfo>,
    selected_device: Option<PeripheralId>,
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
    pub fn new(cmd_tx: UnboundedSender<BleCommand>, evt_rx: Receiver<BleEvent>) -> Self {
        let (config, warning) = Config::load();
        let mut app = Self {
            cmd_tx,
            evt_rx,
            config,
            config_dirty: false,
            scanning: false,
            devices: HashMap::new(),
            selected_device: None,
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
                    self.services.clear();
                    self.selection = Selection::default();
                }
            }
        }
    }

    fn send_text(&mut self, text: &str) {
        if !matches!(self.conn, ConnState::Connected(_)) {
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
        ui.horizontal_wrapped(|ui| {
            if self.scanning {
                if ui.button("⏹ Detener").clicked() {
                    self.command(BleCommand::StopScan);
                }
                ui.spinner();
            } else if ui.button("🔍 Escanear").clicked() {
                self.command(BleCommand::StartScan);
            }

            match &self.conn {
                ConnState::Disconnected => {
                    let enabled = self.selected_device.is_some();
                    if ui
                        .add_enabled(enabled, egui::Button::new("🔗 Conectar"))
                        .clicked()
                    {
                        self.connect_selected();
                    }
                }
                ConnState::Connecting(_) | ConnState::Connected(_) => {
                    if ui.button("❌ Desconectar").clicked() {
                        self.command(BleCommand::Disconnect);
                    }
                }
            }

            ui.separator();
            let status = match &self.conn {
                ConnState::Disconnected => RichText::new("● Desconectado").color(Color32::GRAY),
                ConnState::Connecting(n) => RichText::new(format!("● Conectando a {n}…"))
                    .color(Color32::from_rgb(230, 170, 30)),
                ConnState::Connected(n) => {
                    RichText::new(format!("● {n}")).color(Color32::from_rgb(40, 180, 70))
                }
            };
            ui.label(status);

            ui.separator();
            let mut changed = false;
            changed |= ui.checkbox(&mut self.config.timestamps, "Hora").changed();
            changed |= ui.checkbox(&mut self.config.echo, "Eco").changed();
            self.config_dirty |= changed;

            let log_label = if self.log.is_some() { "⏺ Log" } else { "Log" };
            if ui
                .selectable_label(self.log.is_some(), log_label)
                .on_hover_text("Guardar la sesión en un archivo (ruta automática)")
                .clicked()
            {
                self.toggle_log(None);
            }
            if ui
                .button("📁")
                .on_hover_text("Elegir archivo de log…")
                .clicked()
            {
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
            if ui.button("🗑 Limpiar").clicked() {
                self.lines.clear();
                self.rx_buf.clear();
                self.rx_open = false;
            }
            if ui.button("📋 Copiar").clicked() {
                let text = self.terminal_text();
                ui.ctx().copy_text(text);
            }

            ui.separator();
            ui.toggle_value(&mut self.show_gatt, "Servicios");
            ui.toggle_value(&mut self.show_macro_editor, "Macros");
            ui.toggle_value(&mut self.show_settings, "⚙ Ajustes");
        });
    }

    fn devices_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Dispositivos");
            if ui.small_button("Vaciar").clicked() {
                self.devices.clear();
            }
        });
        if ui.checkbox(&mut self.config.only_nus, "Sólo NUS").changed() {
            self.config_dirty = true;
        }
        ui.separator();

        let mut list: Vec<&DeviceInfo> = self
            .devices
            .values()
            .filter(|d| !self.config.only_nus || d.has_nus)
            .collect();
        list.sort_by(|a, b| {
            b.rssi
                .unwrap_or(i16::MIN)
                .cmp(&a.rssi.unwrap_or(i16::MIN))
                .then_with(|| a.address.cmp(&b.address))
        });

        let mut clicked = None;
        let mut double = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                if list.is_empty() {
                    ui.weak(if self.scanning {
                        "Buscando…"
                    } else {
                        "Pulsa «Escanear»"
                    });
                }
                for d in list {
                    let selected = self.selected_device.as_ref() == Some(&d.id);
                    let name = d.name.as_deref().unwrap_or("(sin nombre)");
                    let mut text = format!("{name}\n{}", d.address);
                    if let Some(r) = d.rssi {
                        text.push_str(&format!("  {r} dBm"));
                    }
                    if d.has_nus {
                        text.push_str("  [NUS]");
                    }
                    let resp = ui.add_sized(
                        [ui.available_width(), 0.0],
                        egui::Button::selectable(selected, text),
                    );
                    if resp.clicked() {
                        clicked = Some(d.id.clone());
                    }
                    if resp.double_clicked() {
                        double = Some(d.id.clone());
                    }
                }
            });
        if let Some(id) = clicked {
            self.selected_device = Some(id);
        }
        if let Some(id) = double {
            self.selected_device = Some(id);
            self.connect_selected();
        }
    }

    fn gatt_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Servicios");
        if self.services.is_empty() {
            ui.weak("Conéctate a un dispositivo para ver sus servicios.");
        } else {
            let mut sel = self.selection;
            char_combo(
                ui,
                "write_combo",
                "Escritura (TX)",
                &self.services,
                &mut sel.write,
                true,
            );
            char_combo(
                ui,
                "notify_combo",
                "Notificación (RX)",
                &self.services,
                &mut sel.notify,
                false,
            );
            if sel != self.selection {
                self.command(BleCommand::Select(sel));
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Usar NUS")
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
                    .button("Guardar como predeterminadas")
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
            ui.label(format!("MTU: {}", self.mtu));
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    for s in &self.services {
                        egui::CollapsingHeader::new(RichText::new(uuid_label(s.uuid)).strong())
                            .id_salt(s.uuid)
                            .default_open(true)
                            .show(ui, |ui| {
                                for c in &s.chars {
                                    let r = CharRef {
                                        service: s.uuid,
                                        uuid: c.uuid,
                                    };
                                    let mut marks = String::new();
                                    if self.selection.write == Some(r) {
                                        marks.push_str(" ✏");
                                    }
                                    if self.selection.notify == Some(r) {
                                        marks.push_str(" 🔔");
                                    }
                                    ui.label(
                                        RichText::new(format!("{}{marks}", uuid_label(c.uuid)))
                                            .monospace(),
                                    );
                                    ui.weak(props_label(c.props));
                                }
                            });
                    }
                });
        }
    }

    fn input_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        // Macros
        ui.horizontal_wrapped(|ui| {
            let mut run = None;
            for (i, m) in self.config.macros.iter().enumerate() {
                let label = if m.name.is_empty() { &m.text } else { &m.name };
                let hint = match m.mode {
                    MacroMode::Send => format!("Enviar: {}", m.text),
                    MacroMode::Insert => format!("Insertar: {}", m.text),
                };
                if ui.button(label).on_hover_text(hint).clicked() {
                    run = Some(i);
                }
            }
            if ui.small_button("✏ Editar").clicked() {
                self.show_macro_editor = true;
            }
            if let Some(i) = run {
                let m = self.config.macros[i].clone();
                self.run_macro(&m);
            }
        });
        ui.add_space(4.0);
        // Entrada
        ui.horizontal(|ui| {
            let mut le = self.config.line_ending;
            egui::ComboBox::from_id_salt("line_ending")
                .selected_text(le.label())
                .width(80.0)
                .show_ui(ui, |ui| {
                    for opt in LineEnding::ALL {
                        ui.selectable_value(&mut le, opt, opt.label());
                    }
                });
            if le != self.config.line_ending {
                self.config.line_ending = le;
                self.config_dirty = true;
            }

            let send_clicked = ui.button("Enviar ➤").clicked();
            let edit = egui::TextEdit::singleline(&mut self.input)
                .hint_text("Escribe un mensaje y pulsa Enter")
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY);
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
        ui.add_space(4.0);
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let ts = self.config.timestamps;
        egui::ScrollArea::both()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show_rows(ui, row_h, self.lines.len(), |ui, range| {
                for line in &self.lines[range] {
                    let mut s = String::new();
                    if ts {
                        s.push_str(&line.time.format("%H:%M:%S%.3f ").to_string());
                    }
                    s.push_str(line.kind.tag());
                    s.push(' ');
                    s.push_str(&line.text);
                    ui.add(
                        egui::Label::new(RichText::new(s).monospace().color(line.kind.color(dark)))
                            .extend(),
                    );
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
        egui::Window::new("Macros")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label("Cambios guardados automáticamente en el archivo de configuración.");
                ui.separator();
                let mut action: Option<(usize, i32)> = None; // (índice, -1 subir / 1 bajar / 0 borrar)
                let len = self.config.macros.len();
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        egui::Grid::new("macro_grid")
                            .striped(true)
                            .num_columns(4)
                            .show(ui, |ui| {
                                ui.strong("Nombre");
                                ui.strong("Texto");
                                ui.strong("Modo");
                                ui.end_row();
                                for (i, m) in self.config.macros.iter_mut().enumerate() {
                                    let mut changed = false;
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut m.name)
                                                .desired_width(110.0),
                                        )
                                        .changed();
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut m.text)
                                                .font(egui::TextStyle::Monospace)
                                                .desired_width(200.0),
                                        )
                                        .changed();
                                    egui::ComboBox::from_id_salt(("macro_mode", i))
                                        .selected_text(match m.mode {
                                            MacroMode::Send => "Enviar",
                                            MacroMode::Insert => "Insertar",
                                        })
                                        .show_ui(ui, |ui| {
                                            changed |= ui
                                                .selectable_value(
                                                    &mut m.mode,
                                                    MacroMode::Send,
                                                    "Enviar",
                                                )
                                                .changed();
                                            changed |= ui
                                                .selectable_value(
                                                    &mut m.mode,
                                                    MacroMode::Insert,
                                                    "Insertar",
                                                )
                                                .changed();
                                        });
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(i > 0, egui::Button::new("⬆").small())
                                            .clicked()
                                        {
                                            action = Some((i, -1));
                                        }
                                        if ui
                                            .add_enabled(
                                                i + 1 < len,
                                                egui::Button::new("⬇").small(),
                                            )
                                            .clicked()
                                        {
                                            action = Some((i, 1));
                                        }
                                        if ui.small_button("🗑").clicked() {
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
                ui.separator();
                if ui.button("➕ Añadir macro").clicked() {
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
        egui::Window::new("Ajustes").open(&mut open).show(ctx, |ui| {
            let mut changed = false;
            egui::Grid::new("settings_grid").num_columns(2).show(ui, |ui| {
                ui.label("Tamaño de trozo (bytes)");
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.config.chunk_size).range(0..=512))
                        .changed();
                    ui.weak(format!(
                        "0 = auto (ahora {})",
                        config::effective_chunk_size(self.config.chunk_size, self.mtu)
                    ));
                });
                ui.end_row();

                ui.label("Escritura con respuesta");
                changed |= ui
                    .checkbox(&mut self.config.write_with_response, "")
                    .on_hover_text("Más lento pero confirmado. Si la característica sólo admite un modo, se usa ese.")
                    .changed();
                ui.end_row();

                ui.label("Escritura predeterminada");
                changed |= uuid_edit(ui, &mut self.config.default_write_uuid);
                ui.end_row();

                ui.label("Notificación predeterminada");
                changed |= uuid_edit(ui, &mut self.config.default_notify_uuid);
                ui.end_row();
            });
            if ui.button("Restaurar UUIDs NUS").clicked() {
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

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("input").show(ui, |ui| self.input_panel(ui));
        egui::Panel::left("devices")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| self.devices_panel(ui));
        if self.show_gatt {
            egui::Panel::right("gatt")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| self.gatt_panel(ui));
        }
        egui::CentralPanel::default_margins().show(ui, |ui| self.terminal(ui));

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

/// Decodifica UTF-8 (con pérdida) y quita el fin de línea.
fn decode_line(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim_end_matches(['\r', '\n']).replace('\r', "\\r")
}

/// UUID con nombre si es conocido.
fn uuid_label(u: Uuid) -> String {
    let name = match u {
        NUS_SERVICE => Some("NUS"),
        NUS_RX_WRITE => Some("NUS RX"),
        NUS_TX_NOTIFY => Some("NUS TX"),
        _ => short_uuid_name(u),
    };
    match name {
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

fn props_label(p: CharPropFlags) -> String {
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
        .collect::<Vec<_>>()
        .join(", ")
}

fn char_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    services: &[ServiceInfo],
    value: &mut Option<CharRef>,
    write: bool,
) {
    ui.label(label);
    let selected = value.map_or("(ninguna)".to_string(), |c| uuid_label(c.uuid));
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "(ninguna)");
            for s in services {
                let chars: Vec<_> = s
                    .chars
                    .iter()
                    .filter(|c| if write { c.can_write() } else { c.can_notify() })
                    .collect();
                if chars.is_empty() {
                    continue;
                }
                ui.weak(uuid_label(s.uuid));
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
    let resp = ui.add(
        egui::TextEdit::singleline(&mut text)
            .font(egui::TextStyle::Monospace)
            .desired_width(300.0),
    );
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
