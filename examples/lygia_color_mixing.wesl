// Lygia Color Mixing — the same two colors blended in RGB, Oklab and as
// paint, then through LYGIA's tonemappers.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, color
// The top three bands mix the same pair of colors left to right: plainly in
// RGB, perceptually in Oklab, and like pigments with Kubelka-Munk spectral
// mixing (blue and yellow make green). The bottom bands push an HDR ramp
// through five of LYGIA's tonemappers (https://lygia.xyz).
import package::shadertoy::{i_time, i_resolution};
import lygia::color::mixOklab::mixOklab;
import lygia::color::mixSpectral::mixSpectral;
import lygia::color::tonemap::aces::tonemapACES3;
import lygia::color::tonemap::reinhard::tonemapReinhard3;
import lygia::color::tonemap::filmic::tonemapFilmic3;
import lygia::color::tonemap::uncharted2::tonemapUncharted23;
import lygia::color::tonemap::unreal::tonemapUnreal3;
import lygia::color::space::hsv2rgb::hsv2rgb;

// How fast the color pair drifts around the hue circle.
const DRIFT: f32 = 0.03; // tweak: 0..0.3
// Peak brightness of the HDR ramp the tonemappers compress.
const PEAK: f32 = 4.0; // tweak: 1..32
const GAP_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);
// The pair starts as spectral.js's blue and yellow (linear RGB), the classic
// case where RGB turns grey and paint turns green.
const COLOR_A: vec3<f32> = vec3<f32>(0.0, 0.015, 0.235);
const COLOR_B: vec3<f32> = vec3<f32>(0.97, 0.64, 0.0);

fn tonemap(i: i32, c: vec3<f32>) -> vec3<f32> {
    switch i {
        case 0: { return tonemapReinhard3(c); }
        // Hejl's filmic curve and Unreal's bake in gamma 2.2: undo it, since
        // the output is sRGB-encoded for us.
        case 1: { return pow(tonemapFilmic3(c), vec3<f32>(2.2)); }
        case 2: { return tonemapACES3(c); }
        case 3: { return tonemapUncharted23(c); }
        default: { return pow(tonemapUnreal3(c), vec3<f32>(2.2)); }
    }
}

/// `c` rotated around the hue circle by `turns`, keeping its luminance roughly.
fn drift(c: vec3<f32>, turns: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735);
    let a = 6.2831853 * turns;
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = frag_coord / res;
    // Eight bands from the top: three mixes, then five tonemappers.
    let band = 7 - i32(floor(uv.y * 8.0));
    let within = fract(uv.y * 8.0);
    let x = uv.x;

    let a = clamp(drift(COLOR_A, DRIFT * i_time()), vec3<f32>(0.0), vec3<f32>(1.0));
    let b = clamp(drift(COLOR_B, DRIFT * i_time()), vec3<f32>(0.0), vec3<f32>(1.0));
    var col: vec3<f32>;
    switch band {
        case 0: { col = mix(a, b, x); }
        case 1: { col = mixOklab(a, b, x); }
        case 2: { col = mixSpectral(a, b, x); }
        default: {
            // An HDR rainbow ramp, 0 to PEAK, through tonemapper band-3.
            let hdr = hsv2rgb(vec3<f32>(fract(0.1 * i_time() + 0.15 * f32(band)), 0.7, 1.0))
                * PEAK * x * x;
            col = tonemap(band - 3, hdr);
        }
    }
    // Thin gaps between the bands, and a thicker one between the two groups.
    let gap = select(0.04, 0.12, band == 3);
    col = mix(GAP_COLOR, col, step(gap, within) * step(within, 1.0 - 0.04));
    return vec4<f32>(col, 1.0);
}
