// Hello Shader — the smallest complete shader: a gradient with rings that
// ripple outward. Start the tutorial here.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: main_image, frag_coord, i_resolution and i_time
//
// The app calls `main_image` once for every pixel, every frame, and shows
// the color it returns. Change a number and press Ctrl+Enter (Cmd+Enter)
// to recompile; a broken edit keeps the last good shader running.

// The app's inputs live in the `package::shadertoy` module; import the
// ones you use (the Imports example has more on this).
import package::shadertoy::{i_resolution, i_time};

// Module-level constants get a control in the Constants window, live while
// you drag (the Tweak Hints example has the details).
const BOTTOM_COLOR: vec3<f32> = vec3<f32>(0.9, 0.45, 0.2);
const TOP_COLOR: vec3<f32> = vec3<f32>(0.1, 0.25, 0.6);
const RINGS: f32 = 6.0; // rings per screen height
const SPEED: f32 = 0.1; // ring speed, screen heights per second

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    // `frag_coord` is the pixel's position in pixels, measured from the
    // bottom-left corner to the pixel's center: the bottom-left pixel is
    // (0.5, 0.5). `i_resolution()` is the size of the view in pixels.
    let res = i_resolution().xy;

    // 0..1 across the view in both directions...
    let uv = frag_coord / res;
    // ...or centered on the middle, in units of the view's height, so that
    // circles stay round whatever the window's shape.
    let p = (frag_coord - 0.5 * res) / res.y;

    // A vertical gradient: BOTTOM_COLOR at uv.y = 0, TOP_COLOR at 1.
    var col = mix(BOTTOM_COLOR, TOP_COLOR, uv.y);

    // `i_time()` counts seconds since the start (or the toolbar's reset),
    // scaled by the speed slider. Subtracting it moves the rings outward.
    let wave = sin(6.2832 * RINGS * (length(p) - SPEED * i_time()));
    col = col * (0.75 + 0.25 * wave);

    // Red, green, blue and alpha, each 0..1 (linear: the app converts to
    // the screen's sRGB on the way out).
    return vec4<f32>(col, 1.0);
}
