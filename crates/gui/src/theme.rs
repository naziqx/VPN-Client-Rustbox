//! Visual theme: font, palettes, spacing and a few shared widgets.
//! UI code takes colors from [`Palette`] instead of hard-coding `Color32`.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Response, RichText, Shadow, Stroke, TextStyle, Theme, Ui, Visuals,
};

/// Adwaita Sans (GNOME's build of Inter, SIL OFL): Cyrillic, arrows and the
/// symbols we use. egui's default fonts stay as fallbacks for icons (⟳, 🔗…).
const FONT: &[u8] = include_bytes!("../assets/fonts/AdwaitaSans-Regular.ttf");

#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub bg: Color32,
    pub sidebar: Color32,
    pub surface: Color32,
    pub border: Color32,
}

const DARK: Palette = Palette {
    accent: Color32::from_rgb(0x4f, 0x8c, 0xff),
    on_accent: Color32::WHITE,
    ok: Color32::from_rgb(0x3d, 0xd6, 0x8c),
    warn: Color32::from_rgb(0xf2, 0xb8, 0x4b),
    bad: Color32::from_rgb(0xf2, 0x6d, 0x6d),
    text: Color32::from_rgb(0xe6, 0xe8, 0xeb),
    muted: Color32::from_rgb(0x94, 0x9a, 0xa4),
    bg: Color32::from_rgb(0x16, 0x17, 0x1b),
    sidebar: Color32::from_rgb(0x1c, 0x1e, 0x23),
    surface: Color32::from_rgb(0x22, 0x25, 0x2b),
    border: Color32::from_rgb(0x2f, 0x33, 0x3a),
};

const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(0x2f, 0x6f, 0xed),
    on_accent: Color32::WHITE,
    ok: Color32::from_rgb(0x16, 0x9b, 0x5a),
    warn: Color32::from_rgb(0xb7, 0x7a, 0x06),
    bad: Color32::from_rgb(0xd1, 0x43, 0x43),
    text: Color32::from_rgb(0x1c, 0x1f, 0x24),
    muted: Color32::from_rgb(0x64, 0x6b, 0x76),
    bg: Color32::from_rgb(0xf6, 0xf7, 0xf9),
    sidebar: Color32::from_rgb(0xee, 0xf0, 0xf3),
    surface: Color32::WHITE,
    border: Color32::from_rgb(0xdd, 0xe1, 0xe6),
};

impl Palette {
    pub fn of(ui: &Ui) -> Self {
        Self::for_dark(ui.visuals().dark_mode)
    }

    pub fn for_ctx(ctx: &egui::Context) -> Self {
        Self::for_dark(ctx.theme() == Theme::Dark)
    }

    fn for_dark(dark: bool) -> Self {
        if dark { DARK } else { LIGHT }
    }

    /// `color` blended over the background: tinted fills for pills and banners.
    pub fn tint(&self, color: Color32, amount: f32) -> Color32 {
        lerp(self.surface, color, amount)
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("adwaita".into(), FontData::from_static(FONT).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family.clone()).or_default();
        // Text font first for proportional; for monospace only as a fallback
        // (Hack stays primary so the log keeps aligned columns).
        match family {
            FontFamily::Proportional => list.insert(0, "adwaita".into()),
            _ => list.push("adwaita".into()),
        }
    }
    ctx.set_fonts(fonts);

    ctx.set_style_of(Theme::Dark, style(&DARK, Visuals::dark()));
    ctx.set_style_of(Theme::Light, style(&LIGHT, Visuals::light()));
}

fn style(p: &Palette, mut v: Visuals) -> egui::Style {
    let mut s = egui::Style {
        text_styles: [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into(),
        ..Default::default()
    };

    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(10.0, 5.0);
    s.spacing.interact_size.y = 28.0;
    s.spacing.window_margin = Margin::same(18);
    s.spacing.menu_margin = Margin::same(6);
    s.spacing.combo_width = 120.0;

    let r = CornerRadius::same(6);
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if v.dark_mode { 110 } else { 40 }),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if v.dark_mode { 90 } else { 30 }),
    };
    v.extreme_bg_color = if v.dark_mode {
        Color32::from_rgb(0x11, 0x12, 0x15)
    } else {
        Color32::WHITE
    };
    v.text_edit_bg_color = Some(v.extreme_bg_color);
    v.faint_bg_color = lerp(p.bg, p.border, 0.35);
    v.selection.bg_fill = lerp(p.bg, p.accent, if v.dark_mode { 0.38 } else { 0.22 });
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.bad;

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (state, fill) in [
        (&mut w.inactive, lerp(p.surface, p.border, 0.55)),
        (&mut w.hovered, lerp(p.surface, p.border, 1.0)),
        (&mut w.active, lerp(p.border, p.accent, 0.35)),
        (&mut w.open, lerp(p.surface, p.border, 0.8)),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = fill;
        state.corner_radius = r;
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.expansion = 0.0;
    }
    // Thin border: text fields stay visible on the dark background.
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.bg_stroke = Stroke::new(1.0, lerp(p.border, p.accent, 0.5));
    w.active.bg_stroke = Stroke::new(1.0, p.accent);

    s.visuals = v;
    s
}

// ── Shared widgets ─────────────────────────────────────────────

/// Filled accent button for the main action on a screen.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    colored_button(ui, text, Palette::of(ui).accent)
}

pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    colored_button(ui, text, Palette::of(ui).bad)
}

fn colored_button(ui: &mut Ui, text: &str, fill: Color32) -> Response {
    let p = Palette::of(ui);
    ui.add(
        egui::Button::new(RichText::new(text).color(p.on_accent).strong())
            .fill(fill)
            .stroke(Stroke::NONE),
    )
}

/// Large accent/red button for connect/disconnect.
pub fn big_button(ui: &mut Ui, text: &str, fill: Color32, enabled: bool) -> Response {
    let p = Palette::of(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).size(15.0).color(p.on_accent).strong())
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(8)
            .min_size(egui::vec2(150.0, 40.0)),
    )
}

/// Small rounded label with a tinted background: protocol, latency, counters.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.tint(color, if ui.visuals().dark_mode { 0.18 } else { 0.12 }))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).color(color).size(12.0));
        })
        .response
}

/// Card: elevated block with a border.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, add)
        .inner
}

/// Muted small-caps section title ("ГРУППЫ", "ПОДПИСКА").
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .color(Palette::of(ui).muted)
            .strong(),
    );
}

pub fn dot(ui: &mut Ui, color: Color32, radius: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(radius * 2.0, radius * 2.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}
