// Fluid — https://www.shadertoy.com/view/XtGcDK by Wyatt Flanders (CC BY-NC-SA 3.0)
// Ported from eliotbo/bevy_shadertoy_wgsl: the compute passes become
// buffer_a (the fluid solver: velocity in .xy, pressure in .z, dye in .w)
// and buffer_d (tracks the mouse drag segment); buffer_b/c are unused.
// Drag the mouse across the view to stir the fluid.

import package::shadertoy::i_channel_0;
import package::shadertoy::i_channel_3;
import package::shadertoy::i_frame;
import package::shadertoy::i_mouse;
import package::shadertoy::i_resolution;

const SOLVER_STEPS: i32 = 2; // tweak: 1..6
// How much pressure survives each frame.
const PRESSURE_DECAY: f32 = 0.95; // tweak: 0.8..0.999
// Mouse stirring: brush radius (px) and top speed of the injected velocity.
const STIR_RADIUS: f32 = 5.; // tweak: 1..30
const STIR_MAX_SPEED: f32 = 20.; // tweak: 0..40
// Radius (px) of the initial puff of dye.
const PUFF_RADIUS: f32 = 20.; // tweak: 0..100
// How darkly the dye shades the white background.
const DYE_CONTRAST: f32 = 3.; // tweak: 0..6
// Frequencies of the sine bands: speed, pressure, dye slope.
const BAND_FREQ: vec3<f32> = vec3<f32>(100., -5., 368.); // tweak: -400..400

// Distance from p to the segment a..b.
fn lnln(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    return length(p - a - (b - a) * clamp(dot(p - a, b - a) / dot(b - a, b - a), 0., 1.));
}

// Bilinear sample of the fluid state at pixel coordinates U.
fn T(U: vec2<f32>) -> vec4<f32> {
    let f = vec2<i32>(floor(U));
    let c = vec2<i32>(ceil(U));
    let fr = fract(U);

    let upleft = vec2<i32>(f.x, c.y);
    let upright = vec2<i32>(c.x, c.y);
    let downleft = vec2<i32>(f.x, f.y);
    let downright = vec2<i32>(c.x, f.y);

    return (1. - fr.x) * (1. - fr.y) * textureLoad(i_channel_0, downleft, 0)
        + (1. - fr.x) * fr.y * textureLoad(i_channel_0, upleft, 0)
        + fr.x * fr.y * textureLoad(i_channel_0, upright, 0)
        + fr.x * (1. - fr.y) * textureLoad(i_channel_0, downright, 0);
}

// Jacobi-ish velocity/pressure solve with semi-Lagrangian advection; the
// mouse segment (from buffer_d) injects velocity, dye is advected in .w.
fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    var U = frag_coord;
    var A = U + vec2<f32>(1., 0.);
    var B = U + vec2<f32>(0., 1.);
    var C = U + vec2<f32>(-1., 0.);
    var D = U + vec2<f32>(0., -1.);
    var u = T(U);
    var a = T(A);
    var b = T(B);
    var c = T(C);
    var d = T(D);
    var p = vec4<f32>(0.);
    var g = vec2<f32>(0.);

    for (var i: i32 = 0; i < SOLVER_STEPS; i = i + 1) {
        U = U - u.xy;
        A = A - a.xy;
        B = B - b.xy;
        C = C - c.xy;
        D = D - d.xy;

        p = p + (vec4<f32>(length(U - A), length(U - B), length(U - C), length(U - D)) - 1.);

        g = g + vec2<f32>(a.z - c.z, b.z - d.z);
        u = T(U);
        a = T(A);
        b = T(B);
        c = T(C);
        d = T(D);
    }

    var Q = u;
    let N = 0.25 * (a + b + c + d);
    Q = mix(Q, N, vec4<f32>(0., 0., 1., 0.));
    let Qxy = Q.xy - (g / 10. / f32(SOLVER_STEPS));
    Q.x = Qxy.x;
    Q.y = Qxy.y;
    Q.z = Q.z + ((p.x + p.y + p.z + p.w) / 10.);
    Q.z = Q.z * PRESSURE_DECAY;

    let mouse = textureLoad(i_channel_3, vec2<i32>(0.5 * R), 0);
    let q = lnln(U, mouse.xy, mouse.zw);
    let m = mouse.xy - mouse.zw;
    let l = length(m);

    if (mouse.z > 0. && l > 0.) {
        let stirred =
            mix(Q.xyw, vec3<f32>(-normalize(m) * min(l, STIR_MAX_SPEED) / 25., 1.), max(0., STIR_RADIUS - q) / 25.);
        Q.x = stirred.x;
        Q.y = stirred.y;
        Q.w = stirred.z;
    }

    // A puff of dye to start with, so the shader is alive without input.
    if (i_frame() < 140u && length(U - 0.5 * R) < PUFF_RADIUS) {
        Q.x = 0.;
        Q.y = 0.1;
        Q.w = 1.;
    }
    // Boundaries: velocity and dye vanish at the edges (pressure survives).
    if (U.x < 1. || U.y < 1. || R.x - U.x < 1. || R.y - U.y < 1.) {
        Q.x = 0.;
        Q.y = 0.;
        Q.w = 0.;
    }
    return Q;
}

// Every pixel of D holds the current mouse segment: this frame's position
// in .xy, the previous frame's in .zw (shadertoy's iMouse semantics).
fn buffer_d(frag_coord: vec2<f32>) -> vec4<f32> {
    let p = textureLoad(i_channel_3, vec2<i32>(frag_coord), 0);
    let m = i_mouse();
    if (m.z > 0.) {
        if (p.z > 0.) {
            return vec4<f32>(m.xy, p.xy);
        }
        return vec4<f32>(m.xy, m.xy);
    }
    return vec4<f32>(-i_resolution().xy, -i_resolution().xy);
}

fn t(v: vec2<f32>) -> vec4<f32> {
    return textureLoad(i_channel_0, vec2<i32>(v), 0);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let U = frag_coord;

    var me = t(U);
    me.z = me.z - 1.;
    var C = 1. - DYE_CONTRAST * me.wwww;

    let d = vec3<f32>(
        t(U + vec2<f32>(1., 0.)).w - t(U - vec2<f32>(1., 0.)).w,
        t(U + vec2<f32>(0., 1.)).w - t(U - vec2<f32>(0., 1.)).w,
        2.,
    );

    let shaded = C.xyz
        - max(
            vec3<f32>(0.),
            sin(BAND_FREQ * vec3<f32>(length(me.xy), me.z, d.y) * me.w),
        );
    C.x = shaded.x;
    C.y = shaded.y;
    C.z = shaded.z;
    C.a = 1.;
    return C;
}
