// Lygia Shapes — LYGIA's 2D distance fields, filled, stroked and eased.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, sdf
// Shapes, drawing helpers, easing curves and color spaces all come from
// LYGIA (https://lygia.xyz), bundled as the `lygia` WESL package. Its 2D
// functions work in 0..1 coordinates centered on (0.5, 0.5).
import package::shadertoy::{i_time, i_resolution};
import lygia::sdf::starSDF::starSDF;
import lygia::sdf::flowerSDF::flowerSDF;
import lygia::sdf::gearSDF::gearSDF;
import lygia::sdf::heartSDF::heartSDF;
import lygia::draw::fill::fill;
import lygia::draw::stroke::stroke;
import lygia::space::rotate::rotate;
import lygia::animation::easing::elasticOut::elasticOut;
import lygia::color::space::hsv2rgb::hsv2rgb;

// Seconds between two pops of the same shape.
const BEAT: f32 = 2.0; // tweak: 0.3..6
const SPIN: f32 = 0.4; // radians per second
const OUTLINE: f32 = 0.03; // tweak: 0..0.1
const SATURATION: f32 = 0.55; // tweak: 0..1
const BACKGROUND_COLOR: vec3<f32> = vec3<f32>(0.015, 0.013, 0.012);
const OUTLINE_COLOR: vec3<f32> = vec3<f32>(0.92, 0.86, 0.7);

/// Shape `i`'s distance field at `st`, and the level its edge sits at.
fn shape(i: i32, st: vec2<f32>) -> vec2<f32> {
    switch i {
        case 0: { return vec2<f32>(starSDF(st, 5, 0.1), 0.3); }
        case 1: { return vec2<f32>(flowerSDF(st, 5), 0.7); }
        // The gear is small at unit scale: zoom in on it.
        case 2: { return vec2<f32>(gearSDF(0.5 + 0.3 * (st - 0.5), 6.0, 10), 0.0); }
        // LYGIA's heart expects y pointing down, frag_coord points up.
        default: { return vec2<f32>(heartSDF(vec2<f32>(st.x, 1.0 - st.y)), 0.0); }
    }
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    // Four square cells side by side, centered vertically.
    let w = res.x / 4.0;
    let i = min(i32(frag_coord.x / w), 3);
    var st = vec2<f32>(frag_coord.x / w - f32(i), (frag_coord.y - 0.5 * res.y) / w + 0.5);

    // Each shape pops in turn: scale up with an elastic overshoot.
    let phase = fract(i_time() / BEAT - 0.25 * f32(i));
    let size = mix(0.35, 1.0, elasticOut(min(phase * 2.0, 1.0)));
    st = rotate(st, SPIN * i_time() * (f32(i) - 1.5));
    st = (st - 0.5) / size + 0.5;

    let d = shape(i, st);
    // LYGIA hues run 0..1 without wrapping around.
    let tint = hsv2rgb(vec3<f32>(fract(0.25 * f32(i) + 0.05 * i_time()), SATURATION, 0.9));
    var col = mix(BACKGROUND_COLOR, tint, fill(d.x, d.y));
    col = mix(col, OUTLINE_COLOR, stroke(d.x, d.y, OUTLINE));
    return vec4<f32>(col, 1.0);
}
