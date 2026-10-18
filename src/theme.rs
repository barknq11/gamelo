//! Colour themes, accent colours and corner style. The active palette lives in
//! a thread-local so drawing code can ask for `pal().text` without threading
//! settings through every widget.

use eframe::egui::{self, Color32};
use std::cell::Cell;

/// Every colour the UI draws with, plus the corner style.
#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub square: bool,
    pub bg: Color32,
    pub sidebar: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card2: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub field: Color32,
    pub text: Color32,
    pub subtext: Color32,
    pub faint: Color32,
    pub accent: Color32,
    /// Text and icons drawn on top of the accent (and danger) colour.
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    /// Alpha of drop shadows (lighter themes want softer shadows).
    pub shadow: u8,
}

impl Palette {
    pub fn info(&self) -> Color32 {
        self.accent
    }
}

thread_local! {
    static PAL: Cell<Palette> = const { Cell::new(SLATE.palette(false)) };
}

/// The palette in use.
pub fn pal() -> Palette {
    PAL.with(|p| p.get())
}

/// Corner radius `n`, or square corners when the square style is on.
pub fn rad(n: u8) -> egui::CornerRadius {
    egui::CornerRadius::same(if pal().square { 0 } else { n })
}

/// A drop-shadow colour at `alpha` as tuned for dark themes (softer on light ones).
pub fn shadow(alpha: f32) -> Color32 {
    Color32::from_black_alpha((alpha * f32::from(pal().shadow) / 150.0).clamp(0.0, 255.0) as u8)
}

/// While alive, `pal()` answers with a dark palette (keeping the accent), for
/// text and badges drawn over artwork, which is always darkened.
pub struct OverImage(Palette);

impl OverImage {
    pub fn new() -> Self {
        let cur = pal();
        if !cur.dark {
            let mut p = SLATE.palette(cur.square);
            p.accent = cur.accent;
            p.on_accent = cur.on_accent;
            PAL.with(|c| c.set(p));
        }
        OverImage(cur)
    }
}

impl Drop for OverImage {
    fn drop(&mut self) {
        PAL.with(|c| c.set(self.0));
    }
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// A named base theme: surfaces and text. Accents may be overridden.
pub struct ThemeDef {
    pub key: &'static str,
    pub name: &'static str,
    pub dark: bool,
    bg: u32,
    sidebar: u32,
    panel: u32,
    card: u32,
    card2: u32,
    hover: u32,
    border: u32,
    field: u32,
    text: u32,
    subtext: u32,
    faint: u32,
    accent: u32,
}

impl ThemeDef {
    pub const fn palette(&self, square: bool) -> Palette {
        let accent = hex(self.accent);
        Palette {
            dark: self.dark,
            square,
            bg: hex(self.bg),
            sidebar: hex(self.sidebar),
            panel: hex(self.panel),
            card: hex(self.card),
            card2: hex(self.card2),
            hover: hex(self.hover),
            border: hex(self.border),
            field: hex(self.field),
            text: hex(self.text),
            subtext: hex(self.subtext),
            faint: hex(self.faint),
            accent,
            on_accent: if self.dark { hex(self.bg) } else { Color32::WHITE },
            ok: if self.dark { hex(0x6FD08C) } else { hex(0x1E8A4C) },
            warn: if self.dark { hex(0xE6B566) } else { hex(0xA0630C) },
            danger: if self.dark { hex(0xEF6F7B) } else { hex(0xC93445) },
            shadow: if self.dark { 150 } else { 40 },
        }
    }

    /// Swatch colours for the theme picker: background, card, accent, text.
    pub fn swatch(&self) -> [Color32; 4] {
        [hex(self.bg), hex(self.card2), hex(self.accent), hex(self.text)]
    }
}

/// The original charcoal theme with an indigo accent.
pub const SLATE: ThemeDef = ThemeDef {
    key: "slate",
    name: "Slate",
    dark: true,
    bg: 0x0D0F14,
    sidebar: 0x101319,
    panel: 0x141821,
    card: 0x1A1F2B,
    card2: 0x222835,
    hover: 0x2A3243,
    border: 0x2A3242,
    field: 0x0A0C11,
    text: 0xE8ECF4,
    subtext: 0xAAB2C4,
    faint: 0x7A8398,
    accent: 0x6D8DFF,
};

pub const THEMES: &[ThemeDef] = &[
    // The brand theme: plum-black with Gamelo rose, from the app icon.
    ThemeDef {
        key: "gamelo",
        name: "Gamelo Night",
        dark: true,
        bg: 0x120D19,
        sidebar: 0x0F0A15,
        panel: 0x181220,
        card: 0x1D1527,
        card2: 0x261C33,
        hover: 0x31243F,
        border: 0x342741,
        field: 0x0C0911,
        text: 0xF7EEF4,
        subtext: 0xC3B2C4,
        faint: 0x8C7B92,
        accent: 0xFF6F98,
    },
    SLATE,
    // Deep navy with a teal accent (theme-factory "Ocean Depths").
    ThemeDef {
        key: "ocean",
        name: "Ocean Depths",
        dark: true,
        bg: 0x0E151F,
        sidebar: 0x111A26,
        panel: 0x16212F,
        card: 0x1A2636,
        card2: 0x223143,
        hover: 0x2A3C51,
        border: 0x2A3B50,
        field: 0x0B1119,
        text: 0xF1FAEE,
        subtext: 0xA8C6CC,
        faint: 0x7792A0,
        accent: 0x48BDB4,
    },
    // Purple-black with a lavender accent ("Midnight Galaxy").
    ThemeDef {
        key: "galaxy",
        name: "Midnight Galaxy",
        dark: true,
        bg: 0x110D19,
        sidebar: 0x15101F,
        panel: 0x1B1528,
        card: 0x211A31,
        card2: 0x2A213D,
        hover: 0x342A4B,
        border: 0x362B4D,
        field: 0x0D0A14,
        text: 0xE6E6FA,
        subtext: 0xB6ACD1,
        faint: 0x857BA2,
        accent: 0xB09CF0,
    },
    // Dark moss with an olive-green accent ("Forest Canopy").
    ThemeDef {
        key: "forest",
        name: "Forest Canopy",
        dark: true,
        bg: 0x0E130E,
        sidebar: 0x111811,
        panel: 0x161F16,
        card: 0x1B261B,
        card2: 0x223022,
        hover: 0x2B3B2A,
        border: 0x2C3B2B,
        field: 0x0B100B,
        text: 0xFAF9F6,
        subtext: 0xBAC1AA,
        faint: 0x878F79,
        accent: 0xA7C77F,
    },
    // Teal-slate with a burnt-orange accent ("Sunset Boulevard").
    ThemeDef {
        key: "sunset",
        name: "Sunset Boulevard",
        dark: true,
        bg: 0x0E171B,
        sidebar: 0x111C21,
        panel: 0x162329,
        card: 0x1B2A31,
        card2: 0x22343C,
        hover: 0x2C414A,
        border: 0x2B3F48,
        field: 0x0B1316,
        text: 0xF6EFE6,
        subtext: 0xC2BBAE,
        faint: 0x8E8C84,
        accent: 0xF08A5D,
    },
    // True black for OLED screens, electric blue ("Tech Innovation").
    ThemeDef {
        key: "oled",
        name: "Tech Black",
        dark: true,
        bg: 0x000000,
        sidebar: 0x060606,
        panel: 0x0E0E0E,
        card: 0x121212,
        card2: 0x1B1B1B,
        hover: 0x262626,
        border: 0x262626,
        field: 0x000000,
        text: 0xF2F2F2,
        subtext: 0xB0B0B0,
        faint: 0x7C7C7C,
        accent: 0x3D8BFF,
    },
    // The brand theme in daylight: blush paper with a deeper rose.
    ThemeDef {
        key: "gamelo",
        name: "Gamelo Day",
        dark: false,
        bg: 0xFBF5F7,
        sidebar: 0xF4EAF0,
        panel: 0xFFFFFF,
        card: 0xFFFFFF,
        card2: 0xF5EAF0,
        hover: 0xEEDDE7,
        border: 0xE7D3DF,
        field: 0xFFFFFF,
        text: 0x2A1830,
        subtext: 0x5F4866,
        faint: 0x927D97,
        accent: 0xD63F6E,
    },
    // Clean grey paper with the indigo accent ("Modern Minimalist").
    ThemeDef {
        key: "paper",
        name: "Paper",
        dark: false,
        bg: 0xF3F4F7,
        sidebar: 0xEAECF1,
        panel: 0xFFFFFF,
        card: 0xFFFFFF,
        card2: 0xEEF0F4,
        hover: 0xE2E6EE,
        border: 0xD7DBE3,
        field: 0xFFFFFF,
        text: 0x1A1E27,
        subtext: 0x4D5664,
        faint: 0x7A8492,
        accent: 0x4659D6,
    },
    // Ice-blue surfaces with a steel-blue accent ("Arctic Frost").
    ThemeDef {
        key: "arctic",
        name: "Arctic Frost",
        dark: false,
        bg: 0xEDF2F9,
        sidebar: 0xE1EAF5,
        panel: 0xFAFAFA,
        card: 0xFAFCFE,
        card2: 0xE8EFF8,
        hover: 0xD9E4F2,
        border: 0xC8D5E6,
        field: 0xFFFFFF,
        text: 0x1B2638,
        subtext: 0x485A72,
        faint: 0x7A889B,
        accent: 0x4A6FA5,
    },
    // Sand and clay with a burgundy ink ("Desert Rose").
    ThemeDef {
        key: "rose",
        name: "Desert Rose",
        dark: false,
        bg: 0xF6EFEA,
        sidebar: 0xEFE4DC,
        panel: 0xFFFBF8,
        card: 0xFFFBF8,
        card2: 0xF2E7DF,
        hover: 0xEADBD0,
        border: 0xE0CDBF,
        field: 0xFFFDFB,
        text: 0x3A1F2D,
        subtext: 0x6E4F5E,
        faint: 0x9B8089,
        accent: 0xA4574A,
    },
    // Cream with a fern-green accent ("Botanical Garden").
    ThemeDef {
        key: "botanical",
        name: "Botanical",
        dark: false,
        bg: 0xF5F3ED,
        sidebar: 0xECE9DF,
        panel: 0xFFFEFA,
        card: 0xFFFEFA,
        card2: 0xEFECE2,
        hover: 0xE4E0D2,
        border: 0xDCD7C6,
        field: 0xFFFFFC,
        text: 0x242E1F,
        subtext: 0x515B49,
        faint: 0x848B79,
        accent: 0x3F7550,
    },
];

/// Accent colours that can replace a theme's own: (key, name, on dark, on light).
pub const ACCENTS: &[(&str, &str, u32, u32)] = &[
    ("indigo", "Indigo", 0x6D8DFF, 0x4659D6),
    ("teal", "Teal", 0x48BDB4, 0x147A72),
    ("violet", "Violet", 0xB09CF0, 0x7048C8),
    ("rose", "Rose", 0xF27E9B, 0xC2385E),
    ("amber", "Amber", 0xF2B45A, 0xA8640A),
    ("green", "Green", 0x7FD08E, 0x2C8A47),
];

pub fn accent_color(key: &str, dark: bool) -> Option<Color32> {
    ACCENTS
        .iter()
        .find(|a| a.0 == key)
        .map(|a| hex(if dark { a.2 } else { a.3 }))
}

pub fn theme_by_key(key: &str, dark: bool) -> &'static ThemeDef {
    THEMES
        .iter()
        .find(|t| t.key == key && t.dark == dark)
        .unwrap_or_else(|| THEMES.iter().find(|t| t.dark == dark).unwrap_or(&SLATE))
}

/// Text/icon colour that reads on top of `c`.
fn readable_on(c: Color32, dark_ink: Color32) -> Color32 {
    let l = 0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b());
    if l > 150.0 { dark_ink } else { Color32::WHITE }
}

/// Build the palette for these choices.
pub fn build(theme_key: &str, dark: bool, accent: &str, square: bool) -> Palette {
    let def = theme_by_key(theme_key, dark);
    let mut p = def.palette(square);
    if let Some(a) = accent_color(accent, dark) {
        p.accent = a;
    }
    let ink = if dark { p.bg } else { hex(0x14161C) };
    p.on_accent = readable_on(p.accent, ink);
    p
}

/// Make `p` current and restyle egui's own widgets to match.
pub fn apply(ctx: &egui::Context, p: Palette) {
    PAL.with(|c| c.set(p));
    ctx.set_theme(if p.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = p.dark;
        v.override_text_color = Some(p.text);
        v.panel_fill = p.bg;
        v.window_fill = p.panel;
        v.window_stroke = egui::Stroke::new(1.0, p.border);
        v.extreme_bg_color = p.field;
        v.text_edit_bg_color = Some(p.field);
        v.faint_bg_color = p.card;
        v.code_bg_color = p.card2;
        v.window_corner_radius = rad(14);
        v.menu_corner_radius = rad(10);
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: Color32::from_black_alpha(p.shadow),
        };
        v.popup_shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(p.shadow),
        };
        v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.40 } else { 0.25 });
        v.selection.stroke = egui::Stroke::new(1.0, p.accent);
        v.hyperlink_color = p.accent;
        v.warn_fg_color = p.warn;
        v.error_fg_color = p.danger;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.card;
        w.noninteractive.weak_bg_fill = p.card;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
        w.noninteractive.corner_radius = rad(10);

        w.inactive.bg_fill = p.card2;
        w.inactive.weak_bg_fill = p.card2;
        // A touch stronger than card borders, so checkboxes and buttons read on cards.
        w.inactive.bg_stroke = egui::Stroke::new(1.0, p.border.lerp_to_gamma(p.subtext, 0.22));
        w.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
        w.inactive.corner_radius = rad(9);

        w.hovered.bg_fill = p.hover;
        w.hovered.weak_bg_fill = p.hover;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent.gamma_multiply(0.9));
        w.hovered.fg_stroke = egui::Stroke::new(1.0, p.text);
        w.hovered.corner_radius = rad(9);

        w.active.bg_fill = p.accent;
        w.active.weak_bg_fill = p.accent;
        w.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
        w.active.fg_stroke = egui::Stroke::new(1.0, p.on_accent);
        w.active.corner_radius = rad(9);

        w.open.bg_fill = p.card2;
        w.open.weak_bg_fill = p.card2;
        w.open.bg_stroke = egui::Stroke::new(1.0, p.border);
        w.open.fg_stroke = egui::Stroke::new(1.0, p.text);
        w.open.corner_radius = rad(9);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_resolves_and_text_contrasts() {
        for t in THEMES {
            let p = build(t.key, t.dark, "", false);
            let l = |c: Color32| 0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b());
            assert!((l(p.text) - l(p.bg)).abs() > 150.0, "{} text contrast", t.name);
            assert!((l(p.subtext) - l(p.card)).abs() > 70.0, "{} subtext contrast", t.name);
        }
        // An unknown key falls back to a theme of the right brightness.
        assert!(!build("nope", false, "", false).dark);
        assert!(build("slate", true, "", true).square);
    }
}
