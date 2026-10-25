//! The engine-free half of the cost heatmap ([`crate::compile`]'s heat
//! mode; the toggle and legend live in the app's `heatmap` module).
//!
//! GPU timer queries cannot attribute cost to a pixel, but loop iterations
//! track it well: a raymarcher spends its steps at silhouettes and in fog,
//! and an fbm its octaves everywhere. [`instrument`] makes every `loop`,
//! `for` and `while` body of the user module count itself in a
//! per-invocation `var<private>` counter, so helper functions and nested
//! loops count too (nested counts multiply naturally). The image entry point
//! ([`MAIN_MODULE_HEAD`]) resets the counter, runs `main_image`, and returns
//! the count through the inferno palette, black at 0 and pale yellow at the
//! legend's max, which is a uniform, so dragging it is live.
//!
//! Like [`crate::tweak`], the rewrite keeps every line where it was (the
//! increment goes right after the loop's `{`, the counter is appended), so
//! diagnostics still point at the lines the user sees.

use crate::tweak::{Tok, lex};

/// The generated per-invocation counter, appended to the user module.
const COUNTER: &str = "st_heat";

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
pub const INFERNO: [[f32; 3]; 7] = [
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

/// The palette at `t` in 0..=1, as sRGB in 0..=1.
pub fn palette(t: f32) -> [f32; 3] {
    std::array::from_fn(|i| {
        INFERNO
            .iter()
            .rev()
            .fold(0.0, |acc, c| acc * t + c[i])
            .clamp(0.0, 1.0)
    })
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
    fn palette_wgsl_parses() {
        let wgsl = palette_wgsl();
        naga::front::wgsl::parse_str(&wgsl).unwrap_or_else(|e| panic!("{e}\n{wgsl}"));
    }
}
