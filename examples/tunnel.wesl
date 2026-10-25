// Tunnel — the classic infinite corridor from the 1/r polar trick.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const SPEED: f32 = 4.0;
const SEGMENTS: f32 = 4.0; // checker tiles per radian around the wall
const FOG: f32 = 0.12; // tweak: 0..0.5
const PALETTE_FREQ: f32 = 0.35; // color cycles along the tunnel
const LAMP_COLOR: vec3<f32> = vec3<f32>(0.6, 0.7, 1.0);
const LAMP_FALLOFF: f32 = 12.0; // larger means a smaller lamp
const LAMP_PULSE: f32 = 3.0; // pulse rate

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    let r = length(uv);
    let a = atan2(uv.y, uv.x);

    let depth = 1.0 / max(r, 0.0005);
    let z = depth + t * SPEED;
    let u = a * SEGMENTS;

    // WESL reserves `mod` for module declarations, so the modulo is spelled out.
    let s = floor(z * 0.5) + floor(u);
    let chk = s - 2.0 * floor(s * 0.5);
    let pal = 0.5 + 0.5 * cos(z * PALETTE_FREQ + vec3<f32>(0.0, 2.0, 4.0));
    var col = pal * (0.25 + 0.75 * chk);
    col = col * exp(-depth * FOG);
    // A lamp pulsing at the far end of the tunnel.
    col = col + LAMP_COLOR * exp(-r * LAMP_FALLOFF) * (0.5 + 0.5 * sin(t * LAMP_PULSE));
    return vec4<f32>(col, 1.0);
}
