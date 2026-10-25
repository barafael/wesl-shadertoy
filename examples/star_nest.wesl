// Star Nest — a fractal star field: space folded and re-lit at every step.
// author: wesl-shadertoy, after Star Nest by Pablo Roman Andrioli (Kali)
// license: MIT OR Apache-2.0
// tags: 3d, fractal
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const SPEED: f32 = 0.15;
const ITERATIONS: i32 = 8; // tweak: 1..20
// The fold offset c circles this point...
const FOLD_CENTER: vec2<f32> = vec2<f32>(0.7, 0.85); // tweak: 0..1.5
// ...at this radius.
const WOBBLE: f32 = 0.1; // tweak: 0..0.5
const GLOW: f32 = 0.006; // tweak: 0..0.03
// Keeps the 1/d^2 spikes finite; smaller means sharper stars.
const GLOW_SOFTNESS: f32 = 0.0008; // tweak: 0.0001..0.005

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    var p = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let c = vec2<f32>(FOLD_CENTER.x + WOBBLE * cos(t), FOLD_CENTER.y + WOBBLE * sin(t));

    // The fold keeps p in a bounded region; each pass adds a 1/d^2 glow
    // spike, which is what makes the fractal stars blow out to white.
    var col = vec3<f32>(0.0);
    for (var k = 0; k < ITERATIONS; k = k + 1) {
        p = abs(p) / clamp(dot(p, p), 0.05, 1.0) - c;
        let d = length(p);
        let glow = GLOW / (d * d + GLOW_SOFTNESS);
        col = col + glow * vec3<f32>(
            0.5 + 0.5 * sin(3.0 * d + t),
            0.5 + 0.5 * sin(4.0 * d + t + 2.0),
            0.5 + 0.5 * sin(5.0 * d + t + 4.0),
        );
    }
    return vec4<f32>(col, 1.0);
}
