// Lygia Palettes — LYGIA's gradient palettes, the Macbeth chart, and the
// four-pigment Zorn palette mixed like paint.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, color, palette
// Left: the gradient palettes in `color::palette` (fire, water, heatmap,
// hue, spectral), with a marker sweeping along them. Top right: the 24 patches of the
// X-Rite ColorChecker (`macbeth`). Bottom right: Anders Zorn's palette of
// white, yellow ochre, cadmium red and ivory black (`zorn`); every pair is
// blended with Kubelka-Munk `mixSpectral`, the way the pigments would mix.
// The charts list sRGB values, decoded to linear with `srgb2rgb` here.
import package::shadertoy::{i_time, i_resolution};
import lygia::color::palette::fire::fire;
import lygia::color::palette::water::water;
import lygia::color::palette::heatmap::heatmap;
import lygia::color::palette::hue::hueDefault;
import lygia::color::palette::spectral::spectral;
import lygia::color::palette::macbeth::macbeth;
import lygia::color::palette::zorn::zorn;
import lygia::color::mixSpectral::mixSpectral;
import lygia::color::space::srgb2rgb::srgb2rgb;

// Seconds for the marker to sweep the strips.
const SWEEP: f32 = 6.0; // tweak: 1..30
// Share of the width taken by the gradient strips.
const SPLIT: f32 = 0.45; // tweak: 0.2..0.8
const GAP: f32 = 4.0; // tweak: 0..16
const BACKGROUND_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);

fn gradient(i: i32, x: f32) -> vec3<f32> {
    switch i {
        // fire's raw curve runs past 1; squash it into the strip.
        case 0: { return clamp(fire(x * 0.9), vec3<f32>(0.0), vec3<f32>(1.0)); }
        case 1: { return water(x); }
        case 2: { return clamp(heatmap(x), vec3<f32>(0.0), vec3<f32>(1.0)); }
        case 3: { return hueDefault(x); }
        default: { return spectral(x); }
    }
}

/// `frag` inside a `size` box with GAP-wide margins: 1 inside, 0 outside.
fn inset(frag: vec2<f32>, size: vec2<f32>) -> f32 {
    let d = min(frag, size - frag);
    return step(0.5 * GAP, min(d.x, d.y));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let split = SPLIT * res.x;
    var col = BACKGROUND_COLOR;

    if frag_coord.x < split {
        // Five horizontal strips, top to bottom.
        let h = res.y / 5.0;
        let row = floor((res.y - frag_coord.y) / h);
        let local = vec2<f32>(frag_coord.x, frag_coord.y - (4.0 - row) * h);
        let x = frag_coord.x / split;
        col = gradient(i32(row), x) * inset(local, vec2<f32>(split, h));
        // The marker, and the color it samples in a band at its foot.
        let at = fract(i_time() / SWEEP);
        let near = abs(x - at) * split;
        col = mix(col, vec3<f32>(1.0), step(near, 1.0) * step(0.2 * h, local.y));
        col = mix(col, gradient(i32(row), at), step(local.y, 0.2 * h) * inset(local, vec2<f32>(split, h)));
    } else if frag_coord.y > 0.45 * res.y {
        // The ColorChecker: 6 × 4 patches, dark skin top left.
        let origin = vec2<f32>(split, 0.45 * res.y);
        let size = vec2<f32>(res.x - split, 0.55 * res.y) / vec2<f32>(6.0, 4.0);
        let p = frag_coord - origin;
        let c = floor(p / size);
        let index = i32(c.x + (3.0 - c.y) * 6.0);
        col = srgb2rgb(macbeth(index)) * inset(p - c * size, size);
    } else {
        // Zorn: pigments on the diagonal, spectral mixes off it.
        let origin = vec2<f32>(split, 0.0);
        let size = vec2<f32>(res.x - split, 0.45 * res.y) / 4.0;
        let p = frag_coord - origin;
        let c = floor(p / size);
        let a = srgb2rgb(zorn(i32(c.x)));
        let b = srgb2rgb(zorn(i32(3.0 - c.y)));
        // Each cell is a small gradient from one pigment into the other.
        let t = (p.x - c.x * size.x) / size.x;
        let mixed = mixSpectral(a, b, mix(0.2, 0.8, t));
        col = select(mixed, a, i32(c.x) == i32(3.0 - c.y)) * inset(p - c * size, size);
    }
    return vec4<f32>(col, 1.0);
}
