// LYGIA 101 — noise, a palette, random dots and a circle, all imported from
// the bundled LYGIA library.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: importing LYGIA functions, and their naming and 0..1 conventions
//
// LYGIA (https://lygia.xyz) ships with the app as the WESL package `lygia`.
// Import a function by its module path, which mirrors LYGIA's folders
// (browse them in vendor/lygia or on lygia.xyz). A name's suffix tells its
// overload: snoise2 takes a vec2, snoise3 a vec3; random2 returns an f32
// from a vec2, random22 a vec2 from a vec2.
import package::shadertoy::{i_resolution, i_time};
import lygia::generative::snoise::snoise3;
import lygia::generative::random::random22;
import lygia::color::palette::spectral::spectral;
import lygia::sdf::circleSDF::circleSDF;
import lygia::draw::stroke::stroke;
import lygia::math::consts::TAU;

const NOISE_SCALE: f32 = 3.0; // tweak: 0.5..10
const DOTS: f32 = 8.0; // tweak: 2..20
const RING_RADIUS: f32 = 0.7; // tweak: 0..1

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    // LYGIA's 2D functions mostly work in 0..1 coordinates ("st"), with
    // shapes centered on (0.5, 0.5). Make the middle square of the view
    // 0..1; the sides go past it.
    let st = (frag_coord - 0.5 * vec2<f32>(res.x - res.y, 0.0)) / res.y;
    let t = i_time();

    // Simplex noise returns -1..1; the palettes take 0..1.
    let n = snoise3(vec3<f32>(st * NOISE_SCALE, 0.2 * t));
    var col = 0.35 * spectral(0.5 + 0.5 * n);

    // random22 gives two values in 0..1 per grid cell: here a dot's offset
    // within its cell and its phase.
    let cell = floor(st * DOTS);
    let r = random22(cell);
    let center = (cell + 0.25 + 0.5 * r) / DOTS;
    let pulse = 0.5 + 0.5 * sin(TAU * r.y + 2.0 * t);
    col += pulse * (1.0 - smoothstep(0.006, 0.012, length(st - center)));

    // circleSDF is 0 at (0.5, 0.5) and 1 on a circle of radius 0.5;
    // stroke draws a line of the given width where it equals RING_RADIUS.
    col += stroke(circleSDF(st), RING_RADIUS, 0.02);
    return vec4<f32>(col, 1.0);
}
