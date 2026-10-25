// Lygia Noise — simplex noise warped by Worley cells, through a spectral palette.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
// The noise, palette and color mixing come from LYGIA (https://lygia.xyz),
// bundled as the `lygia` WESL package: import any of its modules by path.
import package::shadertoy::{i_time, i_resolution};
import lygia::generative::snoise::snoise3;
import lygia::generative::worley::worley2;
import lygia::color::palette::spectral::spectral;
import lygia::color::mixOklab::mixOklab;

const SPEED: f32 = 0.2; // tweak: 0..1
const SCALE: f32 = 2.5; // tweak: 0.5..8
// How far the noise is pushed around by itself and by the cells.
const WARP: f32 = 0.6; // tweak: 0..2
const CELLS: f32 = 5.0; // tweak: 1..16
// Shown along the cell borders, blended in Oklab.
const SHADOW_COLOR: vec3<f32> = vec3<f32>(0.04, 0.02, 0.08);

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let cells = worley2(uv * CELLS + t);
    let warped = uv * SCALE + WARP * vec2<f32>(snoise3(vec3<f32>(uv * SCALE, t)), cells);
    let n = 0.5 + 0.5 * snoise3(vec3<f32>(warped, 0.5 * t));
    let col = mixOklab(SHADOW_COLOR, spectral(clamp(n, 0.0, 1.0)), smoothstep(0.0, 0.6, cells));
    return vec4<f32>(col, 1.0);
}
