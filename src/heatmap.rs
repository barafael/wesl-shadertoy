//! Cost heatmap: the image replaced by a thermal map of how hard each pixel
//! worked. The rewrite, the heat entry point and the palette are in the
//! library ([`heat`]); this is the app side: the toolbar's on/off and the
//! legend in the canvas corner, whose max is a live uniform.
//!
//! Only the image pass shows heat; buffer passes run as usual, so the image
//! still reads the channels it expects. Loops inside imported packages
//! (LYGIA) are not counted, only the editor text's own.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::render::ShaderToyUniform;
use wesl_shadertoy::heat;

/// The legend's default max: iterations that map to the hot end.
pub const DEFAULT_MAX: f32 = 256.0;

/// Heat mode on/off (toolbar flame icon) and the count that maps to the
/// hot end of the palette.
#[derive(Resource)]
pub struct Heat {
    pub on: bool,
    pub max: f32,
}

impl Default for Heat {
    fn default() -> Self {
        Self {
            on: false,
            max: DEFAULT_MAX,
        }
    }
}

/// The palette at `t` in 0..=1, for the legend.
pub fn color(t: f32) -> egui::Color32 {
    let [r, g, b] = heat::palette(t).map(|v| (v * 255.0).round() as u8);
    egui::Color32::from_rgb(r, g, b)
}

/// Upload the legend's max with the other uniforms.
pub fn upload(heat: Res<Heat>, mut uniforms: Single<&mut ShaderToyUniform>) {
    if uniforms.heat_max != heat.max {
        uniforms.heat_max = heat.max;
    }
}

/// The scale, in the bottom-right corner of the canvas: the palette with
/// its counts, and the max to drag.
pub fn legend(ctx: &egui::Context, canvas: egui::Rect, heat: &mut Heat) {
    if !heat.on || !canvas.is_positive() {
        return;
    }
    egui::Area::new(egui::Id::new("heat_legend"))
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(canvas.right_bottom() - egui::vec2(12.0, 12.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label("loop iterations per pixel");
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(200.0, 12.0), egui::Sense::hover());
                let mut mesh = egui::Mesh::default();
                const STEPS: usize = 32;
                for i in 0..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let x = egui::lerp(rect.x_range(), t);
                    mesh.colored_vertex(egui::pos2(x, rect.top()), color(t));
                    mesh.colored_vertex(egui::pos2(x, rect.bottom()), color(t));
                    if i > 0 {
                        let v = 2 * i as u32;
                        mesh.add_triangle(v - 2, v - 1, v);
                        mesh.add_triangle(v - 1, v, v + 1);
                    }
                }
                ui.painter().add(egui::Shape::mesh(mesh));
                ui.horizontal(|ui| {
                    ui.set_width(rect.width());
                    ui.small("0");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small(format!("≥ {:.0}", heat.max));
                    });
                });
                ui.add(
                    egui::Slider::new(&mut heat.max, 1.0..=4096.0)
                        .logarithmic(true)
                        .max_decimals(0)
                        .text("max"),
                )
                .on_hover_text("Iterations that map to the hot end of the palette");
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_runs_from_black_to_pale_yellow() {
        assert_eq!(color(0.0), egui::Color32::from_rgb(0, 0, 0));
        let hot = color(1.0);
        assert!(hot.r() > 240 && hot.g() > 240 && hot.b() > 150, "{hot:?}");
        let mid = color(0.5);
        assert!(mid.r() > mid.g() && mid.r() > mid.b(), "{mid:?}");
    }
}
