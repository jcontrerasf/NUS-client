//! Paleta, estilo global y pequeños componentes visuales reutilizables.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Response, RichText,
    Shadow, Stroke, Style, TextStyle, Theme, ThemePreference, Ui, Visuals, vec2,
};
use egui_phosphor::regular as icon;

/// Familia de fuente con la variante rellena de Phosphor (registrada en `main`).
pub const FILL: &str = "phosphor-fill";

/// Colores de la interfaz para un modo (oscuro o claro).
pub struct Palette {
    /// Fondo de la ventana.
    pub bg: Color32,
    /// Fondo de tarjetas y barras.
    pub surface: Color32,
    /// Superficie elevada / hover.
    pub raised: Color32,
    /// Fondo de campos de texto y del terminal.
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub rx: Color32,
    pub tx: Color32,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(14, 16, 22),
    surface: Color32::from_rgb(23, 26, 34),
    raised: Color32::from_rgb(33, 37, 48),
    field: Color32::from_rgb(10, 12, 17),
    border: Color32::from_rgb(42, 47, 60),
    text: Color32::from_rgb(228, 231, 239),
    text_dim: Color32::from_rgb(136, 144, 163),
    accent: Color32::from_rgb(108, 112, 245),
    on_accent: Color32::WHITE,
    success: Color32::from_rgb(52, 199, 105),
    warn: Color32::from_rgb(245, 166, 35),
    error: Color32::from_rgb(240, 82, 82),
    rx: Color32::from_rgb(86, 214, 140),
    tx: Color32::from_rgb(110, 170, 255),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(242, 244, 248),
    surface: Color32::from_rgb(255, 255, 255),
    raised: Color32::from_rgb(234, 237, 243),
    field: Color32::from_rgb(250, 251, 253),
    border: Color32::from_rgb(220, 224, 232),
    text: Color32::from_rgb(25, 28, 38),
    text_dim: Color32::from_rgb(98, 106, 124),
    accent: Color32::from_rgb(79, 70, 229),
    on_accent: Color32::WHITE,
    success: Color32::from_rgb(22, 150, 72),
    warn: Color32::from_rgb(200, 115, 0),
    error: Color32::from_rgb(214, 45, 45),
    rx: Color32::from_rgb(20, 128, 66),
    tx: Color32::from_rgb(37, 99, 235),
};

impl Palette {
    pub fn of(ui: &Ui) -> &'static Palette {
        if ui.visuals().dark_mode {
            &DARK
        } else {
            &LIGHT
        }
    }
}

/// Mezcla opaca de `a` hacia `b` (t = 0 → a, t = 1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// ------------------------------------------------------------------- estilo

/// Registra los estilos claro y oscuro en el contexto.
pub fn install(ctx: &egui::Context) {
    ctx.set_style_of(Theme::Dark, style(&DARK, true));
    ctx.set_style_of(Theme::Light, style(&LIGHT, false));
}

pub fn set_dark(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
}

fn style(p: &Palette, dark: bool) -> Style {
    let mut style = Style {
        text_styles: [
            (TextStyle::Heading, FontId::proportional(17.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into(),
        ..Default::default()
    };

    let s = &mut style.spacing;
    s.item_spacing = vec2(8.0, 8.0);
    s.button_padding = vec2(10.0, 5.0);
    s.interact_size.y = 26.0;
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);
    s.combo_height = 300.0;
    s.slider_rail_height = 6.0;
    s.scroll.bar_width = 8.0;
    s.scroll.floating = true;

    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r8 = CornerRadius::same(8);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = r8;
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 40 }),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 30 }),
    };
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.raised;
    v.code_bg_color = p.raised;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    v.selection.bg_fill = mix(p.surface, p.accent, if dark { 0.45 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.0, if dark { p.text } else { p.accent });
    v.slider_trailing_fill = true;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r8;

    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r8;

    let hover = mix(p.raised, p.accent, 0.18);
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0, mix(p.border, p.accent, 0.5));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r8;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent;
    w.active.weak_bg_fill = mix(p.raised, p.accent, 0.35);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = r8;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.raised;
    w.open.weak_bg_fill = p.raised;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r8;

    style.visuals = v;
    style
}

// ------------------------------------------------------------- componentes

/// Texto con icono delante.
pub fn label(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// Icono en la variante rellena.
pub fn filled(icon: &str) -> RichText {
    RichText::new(icon).family(FontFamily::Name(FILL.into()))
}

/// Botón principal (relleno con el color de acento).
pub fn accent_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    solid_button(ui, text, Palette::of(ui).accent, true)
}

/// Botón relleno de un color (p.ej. rojo para desconectar).
pub fn solid_button(
    ui: &mut Ui,
    text: impl Into<String>,
    color: Color32,
    enabled: bool,
) -> Response {
    let p = Palette::of(ui);
    let btn = egui::Button::new(RichText::new(text).color(p.on_accent).strong())
        .fill(color)
        .min_size(vec2(0.0, 30.0));
    ui.add_enabled(enabled, btn)
}

/// Botón cuadrado con sólo un icono, sin marco hasta que se pasa el ratón.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(icon).size(17.0))
            .frame_when_inactive(false)
            .min_size(vec2(30.0, 30.0)),
    )
    .on_hover_text(tooltip)
}

/// Igual que [`icon_button`] pero conmuta un booleano y se resalta si está activo.
pub fn icon_toggle(ui: &mut Ui, value: &mut bool, icon: &str, tooltip: &str) -> Response {
    let mut resp = ui
        .add(
            egui::Button::selectable(*value, RichText::new(icon).size(17.0))
                .min_size(vec2(30.0, 30.0)),
        )
        .on_hover_text(tooltip);
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp
}

/// Chip conmutable con icono y texto.
pub fn chip_toggle(ui: &mut Ui, value: &mut bool, icon: &str, text: &str) -> Response {
    let mut resp = ui.add(
        egui::Button::selectable(*value, label(icon, text)).corner_radius(CornerRadius::same(14)),
    );
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp
}

/// Etiqueta redondeada de color.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).strong().color(color));
        })
        .response
}

/// Tarjeta: superficie con borde suave y esquinas redondeadas.
pub fn card_frame(ui: &Ui) -> egui::Frame {
    let p = Palette::of(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
}

/// Título de sección con icono de acento.
pub fn section_title(ui: &mut Ui, icon: &str, title: &str) {
    let p = Palette::of(ui);
    ui.label(RichText::new(icon).size(18.0).color(p.accent));
    ui.label(RichText::new(title).heading().strong());
}

/// Texto secundario pequeño.
pub fn caption(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = Palette::of(ui);
    ui.label(RichText::new(text).small().color(p.text_dim))
}

/// Estado vacío: icono grande centrado, título y pista.
pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, hint: &str) {
    let p = Palette::of(ui);
    ui.add_space(ui.available_height().min(400.0) * 0.18 + 16.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(icon)
                .size(42.0)
                .color(p.text_dim.gamma_multiply(0.7)),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(title).strong().color(p.text));
        ui.label(RichText::new(hint).small().color(p.text_dim));
    });
}

/// Distribuye una fila con contenido alineado a la derecha de altura fija.
pub fn right_aligned<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), height),
        Layout::right_to_left(Align::Center),
        add,
    )
    .inner
}

/// Icono de cobertura y color según el RSSI.
pub fn signal(rssi: Option<i16>, p: &Palette) -> (&'static str, Color32) {
    match rssi {
        None => (icon::CELL_SIGNAL_SLASH, p.text_dim),
        Some(r) if r >= -60 => (icon::CELL_SIGNAL_FULL, p.success),
        Some(r) if r >= -70 => (icon::CELL_SIGNAL_HIGH, p.success),
        Some(r) if r >= -80 => (icon::CELL_SIGNAL_MEDIUM, p.warn),
        Some(r) if r >= -90 => (icon::CELL_SIGNAL_LOW, p.error),
        Some(_) => (icon::CELL_SIGNAL_NONE, p.error),
    }
}
