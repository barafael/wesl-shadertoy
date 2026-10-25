//! Cost heatmap: the image replaced by a thermal map of how hard each pixel
//! worked.
//!
//! GPU timer queries cannot attribute cost to a pixel, but loop iterations
//! track it well: a raymarcher spends its steps at silhouettes and in fog,
//! and an fbm its octaves everywhere. In heat mode [`instrument`] makes every
//! `loop`, `for` and `while` body of the user module count itself in a
//! per-invocation `var<private>` counter, so helper functions and nested
//! loops count too (nested counts multiply naturally). The image entry point
//! ([`MAIN_MODULE_HEAD`]) resets the counter, runs `main_image`, and returns
//! the count through the inferno palette, black at 0 and pale yellow at the
//! legend's max ([`legend`]), which is a uniform, so dragging it is live.
//!
//! Only the image pass shows heat; buffer passes run as usual, so the image
//! still reads the channels it expects. Loops inside imported packages
//! (LYGIA) are not counted, only the editor text's own.
//!
//! Like [`crate::tweak`], the rewrite keeps every line where it was (the
//! increment goes right after the loop's `{`, the counter is appended), so
//! diagnostics still point at the lines the user sees.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::render::ShaderToyUniform;
use crate::tweak::{Tok, lex};

/// The generated per-invocation counter, appended to the user module.
const COUNTER: &str = "st_heat";

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

/// `src` with an increment of the counter at the top of every loop body,
/// plus the counter and the functions [`MAIN_MODULE_HEAD`] reads it with.
pub fn instrument(src: &str) -> String {
    let (tokens, _) = lex(src);
    let text = |i: usize| &src[tokens[i].span.clone()];
    // The body's `{` of every loop: the first brace after the keyword (a
    // `for` header or `while` condition cannot hold one).
    let bodies: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|&(i, t)| t.tok == Tok::Ident && matches!(text(i), "loop" | "for" | "while"))
        .filter_map(|(i, _)| (i + 1..tokens.len()).find(|&j| tokens[j].tok == Tok::Punct(b'{')))
        .map(|j| tokens[j].span.end)
        .collect();
    let mut out = src.to_string();
    for at in bodies.into_iter().rev() {
        out.insert_str(at, &format!(" {COUNTER} += 1u;"));
    }
    out.push_str(&format!(
        "\n// Heat mode: loop iterations of this invocation.\n\
         var<private> {COUNTER}: u32;\n\
         fn st_heat_reset() {{ {COUNTER} = 0u; }}\n\
         fn st_heat_count() -> u32 {{ return {COUNTER}; }}\n"
    ));
    out
}

/// The image entry point in heat mode, replacing
/// `compile::MAIN_MODULE_HEAD` (and flipping `frag_coord` the same way).
pub const MAIN_MODULE_HEAD: &str = r#"import package::shadertoy::FullscreenVertexOutput;
import package::shadertoy::i_resolution;
import package::shadertoy::st_uniforms;
import package::user::main_image;
import package::user::st_load_tweaks;
import package::user::st_heat_reset;
import package::user::st_heat_count;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    st_load_tweaks();
    st_heat_reset();
    let p = in.position.xy;
    _ = main_image(vec2<f32>(p.x, i_resolution().y - p.y));
    let t = f32(st_heat_count()) / max(st_uniforms._heat_max, 1.0);
    return vec4<f32>(st_heat_palette(clamp(t, 0.0, 1.0)), 1.0);
}
"#;

/// Polynomial fit of matplotlib's inferno (sRGB), by Matt Zucker (CC0):
/// coefficients of t^0..t^6 per channel. Shared by the shader and the legend.
const INFERNO: [[f32; 3]; 7] = [
    [0.000_218_940_37, 0.001_651_004_6, -0.019_480_898],
    [0.106_513_42, 0.563_956_4, 3.932_712_4],
    [11.602_493, -3.972_854, -15.942_394],
    [-41.703_995, 17.436_4, 44.354_145],
    [77.162_94, -33.402_36, -81.807_31],
    [-71.319_43, 32.626_064, 73.209_52],
    [25.131_126, -12.242_669, -23.070_326],
];

/// The WGSL palette function, appended to [`MAIN_MODULE_HEAD`]: inferno,
/// decoded from sRGB since the render targets encode it again.
pub fn palette_wgsl() -> String {
    let vec = |c: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", c[0], c[1], c[2]);
    let horner = INFERNO
        .iter()
        .rev()
        .skip(1)
        .fold(vec(INFERNO[6]), |acc, c| {
            format!("{} + t * ({acc})", vec(*c))
        });
    format!(
        "\nfn st_heat_palette(t: f32) -> vec3<f32> {{\n\
         \x20   let srgb = clamp({horner}, vec3<f32>(0.0), vec3<f32>(1.0));\n\
         \x20   return pow(srgb, vec3<f32>(2.2));\n\
         }}\n"
    )
}

/// The palette at `t` in 0..=1, for the legend.
pub fn color(t: f32) -> egui::Color32 {
    let channel = |i: usize| {
        let v = INFERNO.iter().rev().fold(0.0, |acc, c| acc * t + c[i]);
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    egui::Color32::from_rgb(channel(0), channel(1), channel(2))
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

    const SRC: &str = "\
fn f(n: i32) -> i32 {
    var s = 0;
    for (var i = 0; i < n; i++) {
        loop { s += 1; if s > 3 { break; } }
    }
    while s < 10 { s += 2; }
    // for (;;) { never }
    /* while true { never } */
    return s;
}
";

    #[test]
    fn instrument_counts_loop_bodies_and_keeps_lines() {
        let out = instrument(SRC);
        for (before, after) in SRC.lines().zip(out.lines()) {
            if before.contains("for (var") {
                assert_eq!(after, "    for (var i = 0; i < n; i++) { st_heat += 1u;");
            } else if before.contains("loop {") {
                assert!(after.starts_with("        loop { st_heat += 1u; s += 1;"));
                // The `if` body is not a loop.
                assert_eq!(after.matches("st_heat").count(), 1, "{after}");
            } else if before.contains("while s") {
                assert_eq!(after, "    while s < 10 { st_heat += 1u; s += 2; }");
            } else {
                assert_eq!(before, after, "comments and other lines stay put");
            }
        }
        assert!(out.contains("var<private> st_heat: u32;"), "{out}");
        assert!(out.contains("fn st_heat_count() -> u32"), "{out}");
    }

    /// Every example compiles in heat mode, its live constants included.
    #[test]
    fn every_example_compiles_in_heat_mode() {
        for example in crate::examples::ALL {
            let plain = crate::compile::compile_user_shader(example.code)
                .unwrap_or_else(|e| panic!("{}: {e}", example.name));
            let heat = crate::compile::compile_user_shader_with(example.code, true)
                .unwrap_or_else(|e| panic!("{} (heat): {e}", example.name));
            assert_eq!(heat.tweaks, plain.tweaks, "{}", example.name);
            assert!(heat.wgsl.contains("st_heat_palette"), "{}", example.name);
        }
    }

    /// The counter reaches through helper functions, and errors in the
    /// user's code are reported against the plain source.
    #[test]
    fn heat_compile_counts_helpers_and_reports_plain_errors() {
        let src = "\
const STEPS: i32 = 64;
fn march(p: vec2<f32>) -> f32 {
    var d = 0.0;
    for (var i = 0; i < STEPS; i++) { d += length(p); }
    return d;
}
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(march(frag_coord));
}
";
        let wgsl = crate::compile::compile_user_shader_with(src, true)
            .expect("should compile")
            .wgsl;
        assert!(wgsl.contains("+= 1u"), "{wgsl}");
        assert!(wgsl.contains("_heat_max"), "{wgsl}");
        let broken = src.replace("length(p)", "p");
        let msg = crate::compile::compile_user_shader_with(&broken, true)
            .unwrap_err()
            .to_string();
        assert!(!msg.contains("st_heat"), "{msg}");
    }

    #[test]
    fn palette_runs_from_black_to_pale_yellow() {
        assert_eq!(color(0.0), egui::Color32::from_rgb(0, 0, 0));
        let hot = color(1.0);
        assert!(hot.r() > 240 && hot.g() > 240 && hot.b() > 150, "{hot:?}");
        let mid = color(0.5);
        assert!(mid.r() > mid.g() && mid.r() > mid.b(), "{mid:?}");
    }

    #[test]
    fn palette_wgsl_parses() {
        let wgsl = palette_wgsl();
        naga::front::wgsl::parse_str(&wgsl).unwrap_or_else(|e| panic!("{e}\n{wgsl}"));
    }
}
