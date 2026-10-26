// Imports — a wall clock built from small functions, reading the app's
// inputs through imports and a full path. Drag sideways to wind it.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: WESL imports, multi-item imports, and writing your own functions
//
// Your code is the WESL module `package::user`. The app's inputs live in
// the hidden module `package::shadertoy`, LYGIA in the package `lygia` (see
// LYGIA 101). Only what your code uses ends up in the compiled shader.

// Import items by their path. Several from one module go in braces; a
// single one needs none (`import package::shadertoy::i_time;`).
import package::shadertoy::{i_mouse, i_resolution};

const FACE_COLOR: vec3<f32> = vec3<f32>(0.92, 0.9, 0.84);
const HAND_COLOR: vec3<f32> = vec3<f32>(0.12, 0.12, 0.14);
const SECOND_COLOR: vec3<f32> = vec3<f32>(0.85, 0.2, 0.15);
const RADIUS: f32 = 0.4; // tweak: 0.1..0.5
const WIND_HOURS: f32 = 12.0; // hours wound per view width dragged

// A function takes typed parameters and declares what it returns. It can
// be called from above or below its definition.
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return length(p - a - ab * h);
}

// 1 inside a stroke `width` around distance 0, with a soft edge `px` wide.
fn stroke(d: f32, width: f32, px: f32) -> f32 {
    return 1.0 - smoothstep(width - px, width + px, d);
}

// The direction `turns` of a full circle clockwise from 12 o'clock.
fn dial(turns: f32) -> vec2<f32> {
    let a = 6.2832 * turns;
    return vec2<f32>(sin(a), cos(a));
}

// Seconds since local midnight. Instead of importing it, this names
// `i_date` by its full path, which works anywhere without an import.
fn time_of_day() -> f32 {
    return package::shadertoy::i_date().w;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let p = (frag_coord - 0.5 * res) / res.y;
    let px = 1.0 / res.y; // one pixel, for smooth edges

    // While a button is held, wind the clock by how far the pointer moved
    // since the click (see Mouse Input).
    let m = i_mouse();
    let wound = select(0.0, (m.x - m.z) / res.x * WIND_HOURS * 3600.0, m.z > 0.0);
    let s = time_of_day() + wound;

    var col = vec3<f32>(0.2, 0.22, 0.26);
    col = mix(col, FACE_COLOR, stroke(length(p), RADIUS, px));
    // The twelve hour marks: snap the angle to the nearest mark.
    let mark = dial(round(atan2(p.x, p.y) / 6.2832 * 12.0) / 12.0);
    col = mix(col, HAND_COLOR, stroke(segment(p, mark * RADIUS * 0.82, mark * RADIUS * 0.92), 0.006, px));
    // Hour, minute and second hands. The second hand ticks, with a little
    // bounce at the start of each second.
    let bounce = 0.004 * exp(-12.0 * fract(s)) * sin(40.0 * fract(s));
    col = mix(col, HAND_COLOR, stroke(segment(p, vec2<f32>(0.0), dial(s / 43200.0) * RADIUS * 0.5), 0.012, px));
    col = mix(col, HAND_COLOR, stroke(segment(p, vec2<f32>(0.0), dial(s / 3600.0) * RADIUS * 0.75), 0.008, px));
    let second = dial(floor(s) / 60.0 + bounce) * RADIUS * 0.85;
    col = mix(col, SECOND_COLOR, stroke(segment(p, vec2<f32>(0.0), second), 0.003, px));
    col = mix(col, SECOND_COLOR, stroke(length(p), 0.012, px));
    return vec4<f32>(col, 1.0);
}
