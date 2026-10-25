// Perlin — Gustavson's 2D gradient noise (MIT), rolled into fBm and
// domain-warped: the noise field steers its own sample coordinates.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
// The noise is LYGIA's `cnoise2` (https://lygia.xyz), Stefan Gustavson and
// Ian McEwan's classic Perlin noise from webgl-noise.
import package::shadertoy::{i_time, i_resolution};
import lygia::generative::cnoise::cnoise2;

const SPEED: f32 = 0.15;
const OCTAVES: i32 = 5; // tweak: 1..8
const SCALE: f32 = 3.0; // feature size of the landscape
const WARP: f32 = 1.5; // tweak: 0..4
const SEA_COLOR: vec3<f32> = vec3<f32>(0.05, 0.12, 0.24);
const LAND_COLOR: vec3<f32> = vec3<f32>(0.45, 0.35, 0.22);
const SNOW_COLOR: vec3<f32> = vec3<f32>(0.92, 0.90, 0.85);

fn fbm(p: vec2<f32>) -> f32 {
    var value = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < OCTAVES; i = i + 1) {
        value = value + amp * cnoise2(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        amp = amp * 0.5;
    }
    return value;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let warp = vec2<f32>(
        fbm(uv * 2.0 + vec2<f32>(t, -t * 0.7)),
        fbm(uv * 2.0 + vec2<f32>(-t * 0.4, t)),
    );
    let f = fbm(uv * SCALE + WARP * warp + vec2<f32>(0.0, t * 0.5));
    var col = mix(SEA_COLOR, LAND_COLOR, smoothstep(-0.15, 0.25, f));
    col = mix(col, SNOW_COLOR, smoothstep(0.30, 0.55, f));
    col = col * (0.75 + 0.25 * smoothstep(-0.4, 0.4, warp.x + warp.y));
    return vec4<f32>(col, 1.0);
}
