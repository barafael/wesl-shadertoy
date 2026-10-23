//! Gruvbox theming: egui `Visuals` plus the editor's syntax palette.
//!
//! Colors are the official Gruvbox palette (github.com/morhetz/gruvbox):
//! warm retro-groove tones, dark `#282828` / light `#fbf1c7`. The look is
//! deliberately flat — every corner radius and shadow is zero.

use bevy_egui::egui::{self, Color32, CornerRadius, Id, Shadow, Stroke, Visuals};

/// Which Gruvbox flavor the UI is painted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

/// Token colors for the editor and the WGSL viewer (one set per theme).
#[derive(Clone, Copy)]
pub struct Syntax {
    pub comment: Color32,
    pub string: Color32,
    pub number: Color32,
    pub keyword: Color32,
    pub import: Color32,
    pub ty: Color32,
    pub builtin: Color32,
    pub attribute: Color32,
}

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Gruvbox dark: backgrounds, foregrounds, and the *bright* accent row.
mod d {
    pub const BG0_HARD: u32 = 0x1d_2021;
    pub const BG0: u32 = 0x28_2828;
    pub const BG1: u32 = 0x3c_3836;
    pub const BG2: u32 = 0x50_4945;
    pub const BG3: u32 = 0x66_5c54;
    pub const BG4: u32 = 0x7c_6f64;
    pub const FG0: u32 = 0xfb_f4c1;
    pub const FG1: u32 = 0xeb_dbb2;
    pub const FG2: u32 = 0xd5_c4a1;
    pub const FG4: u32 = 0xa8_9984;
    pub const GRAY: u32 = 0x92_8374;
    pub const RED: u32 = 0xfb_4934;
    pub const GREEN: u32 = 0xb8_bb26;
    pub const YELLOW: u32 = 0xfa_bd2f;
    pub const BLUE: u32 = 0x83_a598;
    pub const PURPLE: u32 = 0xd3_8696;
    pub const AQUA: u32 = 0x8e_c07c;
    pub const ORANGE: u32 = 0xfe_8019;
    pub const SELECT: u32 = 0x45_8588;
}

/// Gruvbox light: same names, light backgrounds, *neutral* accent row.
mod l {
    pub const BG0_SOFT: u32 = 0xf9_f5d7;
    pub const BG0: u32 = 0xfb_f1c7;
    pub const BG1: u32 = 0xeb_dbb2;
    pub const BG2: u32 = 0xd5_c4a1;
    pub const BG3: u32 = 0xbd_ae93;
    pub const BG4: u32 = 0xa8_9984;
    pub const FG0: u32 = 0x28_2828;
    pub const FG1: u32 = 0x3c_3836;
    pub const FG2: u32 = 0x50_4945;
    pub const GRAY: u32 = 0x92_8374;
    pub const RED: u32 = 0x9d_0006;
    pub const GREEN: u32 = 0x79_740e;
    pub const YELLOW: u32 = 0xb5_7614;
    pub const BLUE: u32 = 0x07_6678;
    pub const PURPLE: u32 = 0x8f_3f71;
    pub const AQUA: u32 = 0x42_7b58;
    pub const ORANGE: u32 = 0xaf_3b06;
    pub const SELECT: u32 = 0x83_a598;
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }

    /// Follow the OS (`prefers-color-scheme`) in the browser; dark natively.
    pub fn from_system() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let light = web_sys::window()
                .and_then(|w| w.match_media("(prefers-color-scheme: light)").ok())
                .flatten()
                .is_some_and(|m| m.matches());
            if light { Theme::Light } else { Theme::Dark }
        }
        #[cfg(not(target_arch = "wasm32"))]
        Theme::Dark
    }

    pub fn syntax(self) -> Syntax {
        match self {
            Theme::Dark => Syntax {
                comment: rgb(d::GRAY),
                string: rgb(d::GREEN),
                number: rgb(d::PURPLE),
                keyword: rgb(d::RED),
                import: rgb(d::AQUA),
                ty: rgb(d::YELLOW),
                builtin: rgb(d::BLUE),
                attribute: rgb(d::ORANGE),
            },
            Theme::Light => Syntax {
                comment: rgb(l::GRAY),
                string: rgb(l::GREEN),
                number: rgb(l::PURPLE),
                keyword: rgb(l::RED),
                import: rgb(l::AQUA),
                ty: rgb(l::YELLOW),
                builtin: rgb(l::BLUE),
                attribute: rgb(l::ORANGE),
            },
        }
    }

    /// Green for "compiled ok", red for "compile failed".
    pub fn ok_color(self) -> Color32 {
        rgb(match self {
            Theme::Dark => d::GREEN,
            Theme::Light => l::GREEN,
        })
    }

    pub fn err_color(self) -> Color32 {
        rgb(match self {
            Theme::Dark => d::RED,
            Theme::Light => l::RED,
        })
    }

    pub fn visuals(self) -> Visuals {
        match self {
            Theme::Dark => {
                let mut v = Visuals::dark();
                v.panel_fill = rgb(d::BG0);
                v.window_fill = rgb(d::BG0);
                v.window_stroke = Stroke::new(1.0, rgb(d::BG2));
                v.faint_bg_color = rgb(d::BG1);
                v.extreme_bg_color = rgb(d::BG0_HARD);
                v.code_bg_color = rgb(d::BG1);
                v.warn_fg_color = rgb(d::ORANGE);
                v.error_fg_color = rgb(d::RED);
                v.hyperlink_color = rgb(d::BLUE);
                v.weak_text_color = Some(rgb(d::FG4));
                v.selection.bg_fill = rgb(d::SELECT);
                v.selection.stroke = Stroke::new(1.0, rgb(d::FG0));
                apply_widgets(
                    &mut v,
                    W::new(d::BG1, d::BG1, d::BG3, d::FG1),
                    W::new(d::BG2, d::BG1, d::BG4, d::FG2),
                    W::new(d::BG3, d::BG2, d::FG4, d::FG0),
                    W::new(d::BG4, d::BG3, d::FG2, d::FG0),
                );
                apply_flat(&mut v);
                v
            }
            Theme::Light => {
                let mut v = Visuals::light();
                v.panel_fill = rgb(l::BG0);
                v.window_fill = rgb(l::BG0);
                v.window_stroke = Stroke::new(1.0, rgb(l::BG3));
                v.faint_bg_color = rgb(l::BG1);
                v.extreme_bg_color = rgb(l::BG0_SOFT);
                v.code_bg_color = rgb(l::BG1);
                v.warn_fg_color = rgb(l::ORANGE);
                v.error_fg_color = rgb(l::RED);
                v.hyperlink_color = rgb(l::BLUE);
                v.weak_text_color = Some(rgb(l::BG4));
                v.selection.bg_fill = rgb(l::SELECT);
                v.selection.stroke = Stroke::new(1.0, rgb(l::FG0));
                apply_widgets(
                    &mut v,
                    W::new(l::BG1, l::BG1, l::BG3, l::FG1),
                    W::new(l::BG2, l::BG1, l::BG4, l::FG2),
                    W::new(l::BG3, l::BG2, l::FG2, l::FG0),
                    W::new(l::BG4, l::BG3, l::FG1, l::FG0),
                );
                apply_flat(&mut v);
                v
            }
        }
    }
}

/// One widget-state color set (see `egui::WidgetVisuals`).
#[derive(Clone, Copy)]
struct W {
    bg_fill: u32,
    weak_bg_fill: u32,
    bg_stroke: u32,
    fg_stroke: u32,
}

impl W {
    fn new(bg_fill: u32, weak_bg_fill: u32, bg_stroke: u32, fg_stroke: u32) -> Self {
        Self {
            bg_fill,
            weak_bg_fill,
            bg_stroke,
            fg_stroke,
        }
    }
}

fn apply_widgets(v: &mut Visuals, non: W, inact: W, hov: W, act: W) {
    for (dst, src) in [
        (&mut v.widgets.noninteractive, non),
        (&mut v.widgets.inactive, inact),
        (&mut v.widgets.hovered, hov),
        (&mut v.widgets.active, act),
        (&mut v.widgets.open, hov),
    ] {
        dst.bg_fill = rgb(src.bg_fill);
        dst.weak_bg_fill = rgb(src.weak_bg_fill);
        dst.bg_stroke = Stroke::new(1.0, rgb(src.bg_stroke));
        dst.fg_stroke = Stroke::new(1.0, rgb(src.fg_stroke));
        dst.corner_radius = CornerRadius::ZERO;
    }
}

/// No rounded corners, no shadows — the flat gruvbox look.
fn apply_flat(v: &mut Visuals) {
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
}

#[derive(Clone, PartialEq)]
struct Applied(Theme);

/// (Re)apply `theme` to the egui context, but only when it changed.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let id = Id::new("wesl_shadertoy_theme");
    if ctx.data(|d| d.get_temp::<Applied>(id)) == Some(Applied(theme)) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, Applied(theme)));
    // egui 0.36 keeps a style per dark/light slot: paint the matching slot
    // with gruvbox and make it the active theme.
    let egui_theme = match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    };
    ctx.set_visuals_of(egui_theme, theme.visuals());
    ctx.set_theme(egui_theme);
}
