// UI Melt — the shader eats the editor: ripples, drips and color fringes over the live UI.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, ui, post
//
// Switch on the droplet button in the toolbar to feed the UI in as i_ui;
// F2 switches it off again. Clicks still land on the undistorted UI.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;
import package::shadertoy::i_mouse;
import package::shadertoy::i_ui_sample;

const SPEED: f32 = 1.0;
// Ripple height (in screen fractions) and rings per screen height.
const RIPPLE: f32 = 0.003; // tweak: 0..0.03
const RINGS: f32 = 30.0; // tweak: 0..120
// How far the UI drips at the height of each melt, in screen fractions.
const MELT: f32 = 0.08; // tweak: 0..0.4
// Seconds per melt-and-recover cycle.
const MELT_PERIOD: f32 = 12.0; // tweak: 1..60
// Red/blue split along the ripples, in screen fractions.
const ABERRATION: f32 = 0.004; // tweak: 0..0.02
const BACKGROUND: vec3<f32> = vec3<f32>(0.11, 0.07, 0.16); // tweak: color

fn hash(x: f32) -> f32 {
    return fract(sin(x * 127.1) * 43758.5453);
}

// Smooth 1D value noise.
fn noise(x: f32) -> f32 {
    let i = floor(x);
    let f = fract(x);
    return mix(hash(i), hash(i + 1.0), f * f * (3.0 - 2.0 * f));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = frag_coord / res;
    let t = i_time() * SPEED;

    // Slow cosine-palette glow: all there is while the UI mode is off
    // (i_ui is then transparent).
    let p = (frag_coord - 0.5 * res) / res.y;
    let bg = BACKGROUND * (1.0 + 0.35 * cos(t * 0.4 + p.xyx * 2.5 + vec3<f32>(0.0, 2.0, 4.0)));

    // Rings spreading from the last click (the centre until then).
    var center = vec2<f32>(0.5);
    if (i_mouse().z != 0.0) {
        center = abs(i_mouse().zw) / res;
    }
    let d = (uv - center) * vec2<f32>(res.x / res.y, 1.0);
    let r = length(d);
    let dir = d / max(r, 1e-4);
    let wave = sin(r * RINGS * 6.2832 - t * 5.0) * exp(-2.0 * r);
    var warped = uv + dir * vec2<f32>(res.y / res.x, 1.0) * RIPPLE * wave;

    // Columns of uneven width sag downwards and recover: sampling from
    // higher up moves the UI down.
    let phase = 0.5 - 0.5 * cos(6.2832 * t / MELT_PERIOD);
    let drip = pow(noise(uv.x * 18.0), 3.0) + 0.3 * noise(uv.x * 90.0);
    warped.y = warped.y + MELT * phase * drip * smoothstep(0.0, 0.6, 1.0 - uv.y + 0.3);

    // Chromatic aberration along the ripples.
    let split = dir * vec2<f32>(res.y / res.x, 1.0) * ABERRATION * (0.5 + 0.5 * wave);
    let ui_r = i_ui_sample(warped + split);
    let ui_g = i_ui_sample(warped);
    let ui_b = i_ui_sample(warped - split);
    let ui = vec4<f32>(ui_r.r, ui_g.g, ui_b.b, max(ui_g.a, max(ui_r.a, ui_b.a)));

    // The UI is premultiplied: lay it over the background.
    return vec4<f32>(ui.rgb + bg * (1.0 - ui.a), 1.0);
}
