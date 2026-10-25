// Perlin — Gustavson's 2D gradient noise (MIT), rolled into fBm and
// domain-warped: the noise field steers its own sample coordinates.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
import package::shadertoy::{i_time, i_resolution};

const SPEED: f32 = 0.15;
const OCTAVES: i32 = 5; // tweak: 1..8
const SCALE: f32 = 3.0; // feature size of the landscape
const WARP: f32 = 1.5; // tweak: 0..4
const SEA_COLOR: vec3<f32> = vec3<f32>(0.05, 0.12, 0.24);
const LAND_COLOR: vec3<f32> = vec3<f32>(0.45, 0.35, 0.22);
const SNOW_COLOR: vec3<f32> = vec3<f32>(0.92, 0.90, 0.85);

fn permute4(x: vec4<f32>) -> vec4<f32> {
    return ((x * 34.0 + 1.0) * x) % vec4<f32>(289.0);
}

fn fade2(t: vec2<f32>) -> vec2<f32> {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn perlin_noise2(p: vec2<f32>) -> f32 {
    var pi: vec4<f32> = floor(p.xyxy) + vec4<f32>(0.0, 0.0, 1.0, 1.0);
    let pf = fract(p.xyxy) - vec4<f32>(0.0, 0.0, 1.0, 1.0);
    pi = pi % vec4<f32>(289.0); // keep the permutation free of truncation effects
    let i_x = pi.xzxz;
    let i_y = pi.yyww;
    let fx = pf.xzxz;
    let fy = pf.yyww;
    let i = permute4(permute4(i_x) + i_y);
    var gx: vec4<f32> = 2.0 * fract(i * 0.0243902439) - 1.0; // 1/41
    let gy = abs(gx) - 0.5;
    let tx = floor(gx + 0.5);
    gx = gx - tx;
    let g00 = vec2<f32>(gx.x, gy.x);
    let g10 = vec2<f32>(gx.y, gy.y);
    let g01 = vec2<f32>(gx.z, gy.z);
    let g11 = vec2<f32>(gx.w, gy.w);
    let norm = 1.79284291400159 - 0.85373472095314 * vec4<f32>(
        dot(g00, g00), dot(g01, g01), dot(g10, g10), dot(g11, g11),
    );
    let n00 = dot(g00 * norm.x, vec2<f32>(fx.x, fy.x));
    let n10 = dot(g10 * norm.y, vec2<f32>(fx.y, fy.y));
    let n01 = dot(g01 * norm.z, vec2<f32>(fx.z, fy.z));
    let n11 = dot(g11 * norm.w, vec2<f32>(fx.w, fy.w));
    let fade_xy = fade2(pf.xy);
    let n_x = mix(vec2<f32>(n00, n01), vec2<f32>(n10, n11), vec2<f32>(fade_xy.x));
    let n_xy = mix(n_x.x, n_x.y, fade_xy.y);
    return 2.3 * n_xy;
}

fn fbm(p: vec2<f32>) -> f32 {
    var value = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < OCTAVES; i = i + 1) {
        value = value + amp * perlin_noise2(q);
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
