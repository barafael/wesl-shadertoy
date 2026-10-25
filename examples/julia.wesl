// Julia — the Mandelbrot's sibling, with c orbiting the main cardioid.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, fractal
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const MAX_ITER: i32 = 128; // tweak: 16..512
const ORBIT_RADIUS: f32 = 0.7885; // tweak: 0..1.2
const ORBIT_SPEED: f32 = 0.4;
const VIEW_SCALE: f32 = 1.6; // half-height of the view
const PALETTE_FREQ: f32 = 14.0; // color bands across the escape range
const PALETTE_PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let c = ORBIT_RADIUS * vec2<f32>(cos(t * ORBIT_SPEED), sin(t * ORBIT_SPEED));

    var z = uv * VIEW_SCALE;
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
    let col = 0.5 + 0.5 * cos(2.0 + f * PALETTE_FREQ + PALETTE_PHASE);
    return vec4<f32>(col, 1.0);
}
