// Plasma — the classic cosine palette, driven by i_time.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, palette, beginner
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

const SPEED: f32 = 1.0;
const ZOOM: f32 = 1.0; // pattern scale
// Per-channel phase of the cosine palette.
const PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3
const BRIGHTNESS: f32 = 0.5; // tweak: 0..1
const CONTRAST: f32 = 0.5; // tweak: 0..1

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let col = BRIGHTNESS + CONTRAST * cos(t + uv.xyx * ZOOM + PHASE);
    return vec4<f32>(col, 1.0);
}
