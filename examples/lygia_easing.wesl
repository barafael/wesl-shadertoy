// Lygia Easing — twelve of LYGIA's easing curves, plotted and raced.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, animation
// Every curve is a function of LYGIA's `animation::easing` module
// (https://lygia.xyz), mapping 0..1 time to 0..1 progress (some overshoot).
// Each cell plots its curve and moves a dot along it; the bar underneath
// shows the progress the dot would drive.
import package::shadertoy::{i_time, i_resolution};
import lygia::animation::easing::sineInOut::sineInOut;
import lygia::animation::easing::quadraticOut::quadraticOut;
import lygia::animation::easing::cubicInOut::cubicInOut;
import lygia::animation::easing::quinticIn::quinticIn;
import lygia::animation::easing::exponentialInOut::exponentialInOut;
import lygia::animation::easing::circularOut::circularOut;
import lygia::animation::easing::backIn::backIn;
import lygia::animation::easing::backOut::backOut;
import lygia::animation::easing::elasticOut::elasticOut;
import lygia::animation::easing::elasticInOut::elasticInOut;
import lygia::animation::easing::bounceOut::bounceOut;
import lygia::animation::easing::bounceInOut::bounceInOut;
import lygia::color::space::hsv2rgb::hsv2rgb;

// Seconds for one run along the curves, plus a pause at the end.
const DURATION: f32 = 2.0; // tweak: 0.3..6
const PAUSE: f32 = 0.6; // tweak: 0..3
const LINE_WIDTH: f32 = 1.5; // tweak: 0.5..5
const BACKGROUND_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);
const GRID_COLOR: vec3<f32> = vec3<f32>(0.07, 0.066, 0.06);

fn ease(i: i32, t: f32) -> f32 {
    switch i {
        case 0: { return sineInOut(t); }
        case 1: { return quadraticOut(t); }
        case 2: { return cubicInOut(t); }
        case 3: { return quinticIn(t); }
        case 4: { return exponentialInOut(t); }
        case 5: { return circularOut(t); }
        case 6: { return backIn(t); }
        case 7: { return backOut(t); }
        case 8: { return elasticOut(t); }
        case 9: { return elasticInOut(t); }
        case 10: { return bounceOut(t); }
        default: { return bounceInOut(t); }
    }
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    // A 4×3 grid of cells. Each plots its curve with time 0..1 across 70% of
    // the cell and progress -0.5..1.5 (room for overshoot) down its height.
    let cols = 4.0;
    let rows = 3.0;
    let cell = res / vec2<f32>(cols, rows);
    let c = floor(frag_coord / cell);
    let i = i32(c.x + (rows - 1.0 - c.y) * cols);
    // Pixels per unit of time (x) and of progress (y).
    let scale = vec2<f32>(0.7 * cell.x, 0.4 * cell.y);
    let origin = c * cell + vec2<f32>(0.15 * cell.x, 0.27 * cell.y);
    let p = (frag_coord - origin) / scale;

    let tint = hsv2rgb(vec3<f32>(f32(i) / 12.0, 0.6, 0.95));
    var col = BACKGROUND_COLOR;
    // The 0 and 1 lines the overshooting curves cross.
    let across = step(0.0, p.x) * step(p.x, 1.0);
    let guide = min(abs(p.y), abs(p.y - 1.0)) * scale.y;
    col = mix(col, GRID_COLOR, across * (1.0 - smoothstep(0.0, 1.5, guide)));

    // The curve: distance to it in pixels, from its value and slope.
    if across > 0.0 {
        let y = ease(i, p.x);
        let dy = (ease(i, p.x + 0.002) - ease(i, p.x - 0.002)) / 0.004 * scale.y / scale.x;
        let d = abs(p.y - y) * scale.y / sqrt(1.0 + dy * dy);
        col = mix(col, tint * 0.6, 1.0 - smoothstep(0.5 * LINE_WIDTH, 0.5 * LINE_WIDTH + 1.0, d));
    }

    // The dot riding the curve, and the progress bar below.
    let t = clamp((i_time() % (DURATION + PAUSE)) / DURATION, 0.0, 1.0);
    let v = ease(i, t);
    let dot_ = length((p - vec2<f32>(t, v)) * scale);
    col = mix(col, tint, 1.0 - smoothstep(4.0, 5.5, dot_));
    let bar = step(-0.62, p.y) * step(p.y, -0.54) * step(0.0, p.x) * step(p.x, clamp(v, 0.0, 1.2));
    col = mix(col, tint * 0.8, bar);
    return vec4<f32>(col, 1.0);
}
