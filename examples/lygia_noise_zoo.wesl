// Lygia Noise Zoo — eight of LYGIA's generators side by side.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
// From LYGIA's `generative` modules (https://lygia.xyz), left to right:
//   top:    random (white noise per cell), simplex, classic Perlin,
//           periodic Perlin (tiles every REPEAT cells; the lines mark a tile)
//   bottom: Worley F1, Worley F2 − F1 (cell borders), wavelet, and value
//           noise with analytic derivatives, lit as a height field.
// All but random drift through their third dimension over time.
import package::shadertoy::{i_time, i_resolution};
import lygia::generative::random::random2;
import lygia::generative::snoise::snoise3;
import lygia::generative::cnoise::cnoise3;
import lygia::generative::pnoise::pnoise3;
import lygia::generative::worley::{worley3, worley32};
import lygia::generative::wavelet::wavelet3;
import lygia::generative::noised::noised3;

const SCALE: f32 = 4.0; // tweak: 1..16
const SPEED: f32 = 0.3; // tweak: 0..2
const REPEAT: f32 = 2.0; // tweak: 1..6
// How steep the lit height field in the last cell is.
const RELIEF: f32 = 0.6; // tweak: 0..3
const GAP: f32 = 3.0; // tweak: 0..12
const GAP_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);
const LOW_COLOR: vec3<f32> = vec3<f32>(0.02, 0.03, 0.06);
const HIGH_COLOR: vec3<f32> = vec3<f32>(0.95, 0.9, 0.8);

/// Generator `i` at `p` (time in z), as a 0..1 value or a color.
fn zoo(i: i32, p: vec3<f32>) -> vec3<f32> {
    var v = 0.0;
    switch i {
        case 0: { v = random2(floor(p.xy * 8.0)); }
        case 1: { v = 0.5 + 0.5 * snoise3(p); }
        case 2: { v = 0.5 + 0.7 * cnoise3(p); }
        case 3: {
            v = 0.5 + 0.7 * pnoise3(p, vec3<f32>(REPEAT, REPEAT, 1e4));
            // Mark one period, so the seamless wrap is visible.
            let edge = abs(fract(p.xy / REPEAT + 0.5) - 0.5) * REPEAT;
            if min(edge.x, edge.y) < 0.02 {
                return vec3<f32>(0.85, 0.35, 0.2);
            }
        }
        case 4: { v = worley3(p); }
        case 5: {
            let f = worley32(p);
            v = smoothstep(0.0, 0.25, f.y - f.x);
        }
        case 6: { v = 0.5 + 0.5 * wavelet3(p); }
        default: {
            // x is the noise, yzw its gradient: shade it like a heightfield.
            let n = noised3(p);
            let normal = normalize(vec3<f32>(-RELIEF * n.yz, 1.0));
            let light = max(dot(normal, normalize(vec3<f32>(-0.5, 0.6, 0.6))), 0.0);
            return mix(LOW_COLOR, HIGH_COLOR, 0.15 + 0.85 * light) * (0.6 + 0.4 * n.x);
        }
    }
    return mix(LOW_COLOR, HIGH_COLOR, clamp(v, 0.0, 1.0));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let cell = res / vec2<f32>(4.0, 2.0);
    let c = floor(frag_coord / cell);
    let i = i32(c.x + (1.0 - c.y) * 4.0);
    let local = frag_coord - c * cell;
    // Each cell has the same coordinates, so the generators line up.
    let p = vec3<f32>((local - 0.5 * cell) / cell.y * SCALE, SPEED * i_time());
    var col = zoo(i, p);
    let border = min(min(local.x, local.y), min(cell.x - local.x, cell.y - local.y));
    col = mix(GAP_COLOR, col, step(0.5 * GAP, border));
    return vec4<f32>(col, 1.0);
}
