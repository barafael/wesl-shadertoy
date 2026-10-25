// Mouse Input — what i_mouse() reports: drag on the view and watch the
// pointer, the click and the line between them.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: i_mouse: pointer and click positions, and whether a button is held
//
// `i_mouse()` works like shadertoy's iMouse, in the same pixels as
// `frag_coord` (origin bottom-left):
//   xy  the pointer while a button is held, else where it was released.
//       Moving the pointer without a button changes nothing.
//   zw  where the button went down: positive while it is held, negated
//       once it is released. abs(zw) is always the click position.
// All four are 0 until the first click. Clicks on the app's windows do not
// count; in the Examples window, hovering a card drags across it.
import package::shadertoy::{i_mouse, i_resolution};

const BG_COLOR: vec3<f32> = vec3<f32>(0.1, 0.1, 0.13);
const HELD_COLOR: vec3<f32> = vec3<f32>(0.12, 0.2, 0.14); // background while held
const POINTER_COLOR: vec3<f32> = vec3<f32>(1.0, 0.8, 0.3);
const CLICK_COLOR: vec3<f32> = vec3<f32>(0.4, 0.7, 1.0);
const SIZE: f32 = 10.0; // tweak: 2..40

// 1 within `width` pixels of `d` = 0, fading out over a pixel.
fn line(d: f32, width: f32) -> f32 {
    return clamp(width + 0.5 - abs(d), 0.0, 1.0);
}

// Distance from p to the segment a..b.
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - a - ab * h);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var m = i_mouse();
    // Before the first click, pretend there was one in the middle (a
    // common trick, so a shader looks right before anyone touches it).
    if (all(m == vec4<f32>(0.0))) {
        let mid = 0.5 * i_resolution().xy;
        m = vec4<f32>(mid, -mid);
    }
    let pointer = m.xy;
    let click = abs(m.zw);
    let held = m.z > 0.0;

    var col = select(BG_COLOR, HELD_COLOR, held);
    // A grid every 50 pixels, to read positions off.
    let grid = abs(fract(frag_coord / 50.0 + 0.5) - 0.5) * 50.0;
    col += 0.04 * line(min(grid.x, grid.y), 0.5);

    // The drag, from the click to the pointer; dashed once released.
    let dash = select(step(0.5, fract(length(frag_coord - click) / 12.0)), 1.0, held);
    col = mix(col, vec3<f32>(0.8), 0.6 * dash * line(segment(frag_coord, click, pointer), 1.0));
    // A ring where the button went down, a dot at the pointer.
    col = mix(col, CLICK_COLOR, line(length(frag_coord - click) - SIZE, 1.5));
    col = mix(col, POINTER_COLOR, clamp(SIZE * 0.6 + 0.5 - length(frag_coord - pointer), 0.0, 1.0));
    return vec4<f32>(col, 1.0);
}
