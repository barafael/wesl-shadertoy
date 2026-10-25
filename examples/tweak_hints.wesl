// Tweak Hints — every kind of live constant, and the trailing comments that
// shape their controls in the Constants window.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: live constants and their tweak hints: ranges, colors, integers, off
//
// Open the Constants window. Every module-level `const` whose value is a
// plain literal (a number, or a vecN of numbers) gets a control there.
// Dragging it updates the running shader at once, without a recompile, and
// writes the new value into the code when you let go.
import package::shadertoy::{i_resolution, i_time};

// No hint: a slider from 0 to twice the value (from -2x to 0 for a negative
// value, -1..1 for zero). You can still type in a value outside it.
const SPEED: f32 = 0.5;

// `tweak: lo..hi` picks the slider's range.
const STRIPES: f32 = 12.0; // tweak: 1..40

// Integer constants need their type: an untyped `5` could be an i32, a u32
// or an f32 wherever it is used, so it gets no control. Typed, its slider
// moves in whole steps.
const SPOTS: i32 = 5; // tweak: 1..12

// A vec3 or vec4 whose name contains `color`, `col` or `tint` gets a color
// picker by itself...
const BG_COLOR: vec3<f32> = vec3<f32>(0.08, 0.1, 0.16);
// ...and `tweak: color` asks for one whatever the name.
const GLOW: vec3<f32> = vec3<f32>(1.0, 0.75, 0.3); // tweak: color

// Other vectors get one field per component; a range applies to each.
const CENTER: vec2<f32> = vec2<f32>(0.0, 0.0); // tweak: -0.5..0.5

// `tweak: off` leaves a constant out of the window. It is also how to say
// "fixed on purpose": RING is used in another constant's value below, where
// WGSL needs a true constant, so it can never be live. Without the `off`
// the window would list it as fixed.
const RING: f32 = 0.3; // tweak: off
// Not a plain literal (it is computed), so it never gets a control.
const SPOT_ORBIT: f32 = RING * 1.2;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let p = (frag_coord - 0.5 * res) / res.y - CENTER;
    let t = i_time() * SPEED;

    // Faint diagonal stripes, STRIPES per screen height.
    var col = BG_COLOR * (1.0 + 0.3 * sin(6.2832 * STRIPES * (p.x + p.y) - 4.0 * t));

    // A glowing ring of radius RING...
    col += GLOW * 0.004 / abs(length(p) - RING);

    // ...circled by SPOTS spots.
    for (var i = 0; i < SPOTS; i++) {
        let a = t + 6.2832 * f32(i) / f32(SPOTS);
        let spot = SPOT_ORBIT * vec2<f32>(cos(a), sin(a));
        col += GLOW * 0.0004 / dot(p - spot, p - spot);
    }
    return vec4<f32>(col, 1.0);
}
