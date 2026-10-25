//! Gruvbox dark theming: egui `Visuals` plus the editor's syntax palette.
//!
//! Colors are the official Gruvbox palette (github.com/morhetz/gruvbox):
//! warm retro-groove tones on a `#282828` background. The look is
//! deliberately flat — every corner radius and shadow is zero.

use bevy_egui::egui::{self, Color32, CornerRadius, Id, Shadow, Stroke, Visuals};

/// Token colors for the editor and the WGSL viewer.
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

pub const SYNTAX: Syntax = Syntax {
    comment: rgb(d::GRAY),
    string: rgb(d::GREEN),
    number: rgb(d::PURPLE),
    keyword: rgb(d::RED),
    import: rgb(d::AQUA),
    ty: rgb(d::YELLOW),
    builtin: rgb(d::BLUE),
    attribute: rgb(d::ORANGE),
};

/// Green for "compiled ok".
pub const OK_COLOR: Color32 = rgb(d::GREEN);

/// Red for "compile failed".
pub const ERR_COLOR: Color32 = rgb(d::RED);

pub fn visuals() -> Visuals {
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
        // Same in every state, and equal to the stroke width: egui pads a
        // frameless button (an unselected toggle at rest) by `padding +
        // expansion - stroke` but a framed one by `padding`, so anything else
        // makes toggles grow on hover and shove the rest of the toolbar.
        // Being the same in every state, it also keeps checkboxes and
        // button outlines from swelling under the pointer.
        dst.expansion = dst.bg_stroke.width;
    }
}

/// No rounded corners, no shadows — the flat gruvbox look.
fn apply_flat(v: &mut Visuals) {
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
}

/// Apply the theme to the egui context, once (later calls are no-ops).
pub fn apply(ctx: &egui::Context) {
    let id = Id::new("wesl_shadertoy_theme");
    if ctx.data(|d| d.get_temp::<bool>(id)).is_some() {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, true));
    // egui 0.36 keeps a style per dark/light slot: paint the dark slot with
    // gruvbox and make it the active one (egui may follow a light OS theme).
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
    ctx.set_theme(egui::Theme::Dark);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toggle keeps its size when hovered, so the toolbar does not shift.
    #[test]
    fn hover_does_not_resize_toggles() {
        let ctx = egui::Context::default();
        let rect_with = |pointer: Option<egui::Pos2>| {
            let mut rect = egui::Rect::NOTHING;
            for _ in 0..3 {
                let events = pointer.map(egui::Event::PointerMoved).into_iter().collect();
                let input = egui::RawInput {
                    events,
                    ..Default::default()
                };
                ctx.run_ui(input, |ui| {
                    apply(ui.ctx());
                    ui.horizontal(|ui| {
                        rect = ui.toggle_value(&mut false, "WGSL").rect;
                        let _ = ui.button("?");
                    });
                })
                .drop_without_applying_deltas();
            }
            rect
        };
        let at_rest = rect_with(None);
        let hovered = rect_with(Some(at_rest.center()));
        assert_eq!(at_rest, hovered);
    }
}
