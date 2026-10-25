// Metaballs — implicit 1/r² blobs that fuse, with a fresnel-ish glow.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const BALLS: i32 = 7; // tweak: 1..16
const SPEED: f32 = 1.0;
const ORBIT: f32 = 0.45; // tweak: 0..1
const STRENGTH: f32 = 0.02; // tweak: 0..0.1
const BG_COLOR: vec3<f32> = vec3<f32>(0.02, 0.03, 0.08);
const GLOW_COLOR: vec3<f32> = vec3<f32>(0.1, 0.5, 0.9);
const CORE_COLOR: vec3<f32> = vec3<f32>(0.9, 0.98, 1.0);

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;

    var f = 0.0;
    for (var i = 0; i < BALLS; i = i + 1) {
        let a = t * (0.5 + 0.1 * f32(i)) + f32(i) * 2.3;
        let c = vec2<f32>(sin(a), cos(a * 1.31)) * ORBIT;
        let d = max(dot(uv - c, uv - c), 0.0001);
        f = f + STRENGTH / d;
    }

    let glow = smoothstep(0.35, 0.95, f);
    let inside = smoothstep(0.9, 1.05, f);
    let col = BG_COLOR
        + GLOW_COLOR * glow * 0.6
        + CORE_COLOR * inside;
    return vec4<f32>(col, 1.0);
}
