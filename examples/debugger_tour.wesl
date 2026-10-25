// Debugger Tour — patches of known values to read back with the pixel
// debugger. Turn it on with the toolbar's bug icon, then click around.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, beginner, tutorial
// teaches: the debugger overlay: probe boxes, crosshair, click ring, readouts
//
// The debugger (toolbar bug icon, or `--debugger` on the command line)
// draws over the image without changing your shader:
//   top right      the view's size in pixels; under it two probe boxes with
//                  the r, g, b, a your shader returned at the last click
//                  (upper) and under the pointer (lower), then a swatch of
//                  the clicked pixel's color
//   bottom right   the click position above the pointer position, as in
//                  i_mouse() (see Mouse Input)
//   bottom left    frames per second
//   on the image   a crosshair at the pointer and a ring at the click
// The probe shows the values themselves, also those the screen cannot: try
// the patches below. (On WebGL2 without float render targets they are
// clamped to 0..1 and rounded to 8 bits.)
import package::shadertoy::{i_resolution, i_time};

const SWATCH_COLOR: vec3<f32> = vec3<f32>(0.25, 0.5, 0.75);
const BRIGHT: f32 = 4.0; // tweak: 0..10
const NEGATIVE: f32 = -0.5; // tweak: -2..0
const ALPHA: f32 = 0.25; // tweak: 0..1

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = frag_coord / res;

    // Top half: red is uv.x and green is uv.y, so the lower probe box reads
    // the pointer's position as fractions of the view.
    if (uv.y > 0.5) {
        return vec4<f32>(uv.x, uv.y, 0.0, 1.0);
    }

    // Bottom half: five patches, left to right. Thin gaps between them.
    let column = floor(uv.x * 5.0);
    if (fract(uv.x * 5.0) < 0.01 || uv.y > 0.49) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    switch (i32(column)) {
        // A constant: pick a color in the Constants window, read it back.
        case 0: {
            return vec4<f32>(SWATCH_COLOR, 1.0);
        }
        // Brighter than white: the screen shows white, the probe BRIGHT.
        case 1: {
            return vec4<f32>(BRIGHT, BRIGHT, BRIGHT, 1.0);
        }
        // Below black: the screen shows black, the probe NEGATIVE.
        case 2: {
            return vec4<f32>(NEGATIVE, 0.0, 0.0, 1.0);
        }
        // Alpha: nothing blends with it here, but the probe's a line
        // shows ALPHA.
        case 3: {
            return vec4<f32>(0.5, 0.5, 0.5, ALPHA);
        }
        // Moving: watch the numbers change, or freeze them with the
        // toolbar's speed slider at 0.
        default: {
            return vec4<f32>(vec3<f32>(0.5 + 0.5 * sin(i_time())), 1.0);
        }
    }
}
