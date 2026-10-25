// Buffers 101 — a fading trail: a buffer that remembers the last frame.
// Drag to paint; let go and a dot paints on its own.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: buffer_a feedback, reading the previous frame, i_frame
//
// Define `buffer_a` (up to `buffer_d`) next to `main_image` and the app
// renders it every frame, before the image, into a texture of its own the
// size of the view. Everyone reads that texture as `i_channel_0` (B is
// `i_channel_1`, and so on). A buffer reading its own channel sees what it
// wrote the frame before: that is how a shader keeps state.
import package::shadertoy::{i_channel_0, i_frame, i_mouse, i_resolution, i_time};

const FADE: f32 = 0.97; // tweak: 0.8..1
const BRUSH_RADIUS: f32 = 14.0; // tweak: 2..60
const TRAIL_COLOR: vec3<f32> = vec3<f32>(0.2, 0.6, 1.0);
const HEAD_COLOR: vec3<f32> = vec3<f32>(1.0, 0.9, 0.6);

// Where the brush is: the pointer while a button is held, else a dot
// circling the middle.
fn brush() -> vec2<f32> {
    let m = i_mouse();
    if (m.z > 0.0) {
        return m.xy;
    }
    let t = i_time();
    return 0.5 * i_resolution().xy + 0.3 * i_resolution().y * vec2<f32>(cos(t), sin(2.0 * t));
}

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    // Buffers start out black, and are cleared again on every compile, on
    // Reset and on resize. Anything else to set up goes on frame 0: here,
    // a ring that fades away like the rest of the trail.
    if (i_frame() == 0u) {
        let r = length(frag_coord - 0.5 * i_resolution().xy) - 0.4 * i_resolution().y;
        return vec4<f32>(select(0.0, 1.0, abs(r) < 3.0));
    }
    // Last frame's value at this pixel. A buffer's `frag_coord` is the
    // texel it writes, so reading that same texel lines up; buffers hold
    // 32-bit floats per channel, any value, not just 0..1.
    let last = textureLoad(i_channel_0, vec2<i32>(frag_coord), 0).x;
    let paint = 1.0 - smoothstep(BRUSH_RADIUS - 2.0, BRUSH_RADIUS, length(frag_coord - brush()));
    return vec4<f32>(max(last * FADE, paint));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    // The image reads the buffer at its own `frag_coord`, so buffer texels
    // and screen pixels match one to one, and `i_mouse()` lines up with
    // both. Here it reads this frame's buffer_a, already drawn.
    let v = textureLoad(i_channel_0, vec2<i32>(frag_coord), 0).x;
    let col = TRAIL_COLOR * v + HEAD_COLOR * smoothstep(0.9, 1.0, v);
    return vec4<f32>(col, 1.0);
}
