// Mandelbrot — breathing zoom into the seahorse valley.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, fractal
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const MAX_ITER: i32 = 128; // tweak: 16..512
const ZOOM_SPEED: f32 = 0.3;
// How far the zoom breathes, in e-folds either side of the middle.
const ZOOM_DEPTH: f32 = 5.0; // tweak: 0..8
const CENTER: vec2<f32> = vec2<f32>(-0.745, 0.113); // tweak: -2..1
const PALETTE_FREQ: f32 = 12.0; // color bands across the escape range
const PALETTE_PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let zoom = exp(sin(t * ZOOM_SPEED) * ZOOM_DEPTH + 3.0);
    let c = CENTER + (uv * 1.5) / zoom;

    var z = vec2<f32>(0.0, 0.0);
    var iters = 0.0;
    for (var k = 0; k < MAX_ITER; k = k + 1) {
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        if (dot(z, z) > 4.0) {
            break;
        }
        iters = iters + 1.0;
    }

    if (iters > f32(MAX_ITER) - 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let f = iters / f32(MAX_ITER);
    let col = 0.5 + 0.5 * cos(3.0 + f * PALETTE_FREQ + PALETTE_PHASE);
    return vec4<f32>(col, 1.0);
}
