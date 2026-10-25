// Lygia Post-processing — one scene in buffer A, six of LYGIA's filters
// over it in the image pass. Drag for a lens on the original.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, color, buffers, mouse
// buffer_a draws a small scene of shapes and noise; the image pass splits
// the screen into six panels, each running a different LYGIA filter
// (https://lygia.xyz) on the part of the scene behind it:
//   top:    Prewitt edges, adaptive sharpening, levels
//   bottom: white balance, vibrance, RYB hue shift
// The texture filters read the buffer through `i_sampler`; the color ones
// work per pixel. Their settings sway back and forth over time.
import package::shadertoy::{i_time, i_resolution, i_mouse};
import package::shadertoy::{i_channel_0, i_sampler};
import lygia::filter::edge::prewitt::edgePrewitt;
import lygia::filter::sharpen::adaptive::sharpenAdaptive4;
import lygia::color::levels::levels3Float;
import lygia::color::whiteBalance::whiteBalance3;
import lygia::color::vibrance::vibrance3;
import lygia::color::hueShiftRYB::hueShiftRYB;
import lygia::sdf::starSDF::starSDF;
import lygia::sdf::flowerSDF::flowerSDF;
import lygia::sdf::circleSDF::circleSDF;
import lygia::draw::fill::fill;
import lygia::draw::stroke::stroke;
import lygia::generative::snoise::snoise3;
import lygia::color::mixOklab::mixOklab;
import lygia::space::rotate::rotate;

// Seconds for the filter settings to sway once.
const SWAY: f32 = 6.0; // tweak: 1..30
const SHARPEN: f32 = 1.5; // tweak: 0..4
const LENS_RADIUS: f32 = 0.18; // tweak: 0.05..0.5
const GAP: f32 = 3.0; // tweak: 0..12
const SKY_COLOR: vec3<f32> = vec3<f32>(0.25, 0.45, 0.75);
const DUSK_COLOR: vec3<f32> = vec3<f32>(0.95, 0.55, 0.35);
const STAR_COLOR: vec3<f32> = vec3<f32>(0.98, 0.82, 0.25);
const FLOWER_COLOR: vec3<f32> = vec3<f32>(0.85, 0.25, 0.45);
const RING_COLOR: vec3<f32> = vec3<f32>(0.2, 0.7, 0.55);
const INK_COLOR: vec3<f32> = vec3<f32>(0.05, 0.04, 0.06);
const GAP_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);

/// A shape's 0..1 coordinates: `size` across, centered at `at`.
fn place(st: vec2<f32>, at: vec2<f32>, size: f32) -> vec2<f32> {
    return (st - at) / size + 0.5;
}

// The scene, unfiltered. Buffers are stored y down: flip to y up.
fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let st = vec2<f32>(frag_coord.x, res.y - frag_coord.y) / res.y;
    let t = i_time();
    let aspect = res.x / res.y;

    // A textured gradient backdrop.
    let grain = snoise3(vec3<f32>(st * 18.0, 0.1 * t));
    var col = mixOklab(DUSK_COLOR, SKY_COLOR, st.y + 0.08 * grain);
    col *= 0.97 + 0.03 * snoise3(vec3<f32>(st * 90.0, 0.0));

    // Two rows of shapes across the width, so every panel has some.
    for (var i = 0; i < 12; i++) {
        let fi = f32(i);
        let row = f32(i / 6);
        let at = vec2<f32>((f32(i % 6) + 0.5 + 0.5 * row) / 6.0 * aspect - 0.1,
                           0.27 + 0.46 * row + 0.08 * sin(0.7 * t + 1.9 * fi));
        let q = rotate(place(st, at, 0.3), 0.3 * t * select(1.0, -1.0, i % 2 == 0));
        var d: f32;
        var tint: vec3<f32>;
        switch i % 3 {
            case 0: { d = starSDF(q, 5, 0.1); tint = STAR_COLOR; }
            case 1: { d = flowerSDF(q, 6); tint = FLOWER_COLOR; }
            default: { d = circleSDF(q); tint = RING_COLOR; }
        }
        let level = select(0.6, 0.35, i % 3 == 0);
        col = mix(col, tint, fill(d, level));
        col = mix(col, INK_COLOR, stroke(d, level, 0.03));
    }
    return vec4<f32>(col, 1.0);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = vec2<f32>(frag_coord.x, res.y - frag_coord.y) / res;
    let pixel = 1.0 / res;
    let s = sin(6.2831853 * i_time() / SWAY);
    let original = textureLoad(i_channel_0, vec2<i32>(uv * res), 0).rgb;

    let cell = res / vec2<f32>(3.0, 2.0);
    let c = floor(frag_coord / cell);
    let i = i32(c.x + (1.0 - c.y) * 3.0);
    var col: vec3<f32>;
    switch i {
        case 0: { col = edgePrewitt(i_channel_0, i_sampler, uv, pixel * (1.5 + s)); }
        case 1: { col = sharpenAdaptive4(i_channel_0, i_sampler, uv, pixel, SHARPEN * (1.0 + s)).rgb; }
        case 2: { col = levels3Float(original, 0.15 + 0.1 * s, 1.0 + 0.6 * s, 0.85, 0.0, 1.0); }
        case 3: { col = whiteBalance3(original, 0.6 * s, 0.3 * cos(3.1415927 * s)); }
        case 4: { col = vibrance3(original, 1.5 * s); }
        default: { col = hueShiftRYB(original, 3.1415927 * s); }
    }

    // Dragging opens a lens onto the unfiltered scene.
    let mouse = i_mouse();
    if mouse.z > 0.0 {
        let d = length(frag_coord - mouse.xy) / res.y;
        col = mix(col, original, step(d, LENS_RADIUS));
        col = mix(col, vec3<f32>(1.0), step(abs(d - LENS_RADIUS), 1.0 / res.y));
    }
    let local = frag_coord - c * cell;
    let border = min(min(local.x, local.y), min(cell.x - local.x, cell.y - local.y));
    col = mix(GAP_COLOR, col, step(0.5 * GAP, border));
    return vec4<f32>(col, 1.0);
}
