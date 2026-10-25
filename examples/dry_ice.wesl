// Dry Ice — fog simulated in four buffer passes, raymarched around bouncing balls.
// author: David Gallardo
// license: CC BY-NC-SA 3.0
// source: https://www.shadertoy.com/view/WlVyRV
// tags: 3d, fluid, volumetric
// Ported from eliotbo/bevy_shadertoy_wgsl: the four compute passes become
// buffer_a (2D fog velocity/density sim), buffer_b (velocity divergence),
// buffer_c + buffer_d (a Jacobi pressure solve whose result feeds back into
// buffer_a's velocity as a projection), and the image pass raymarches the fog
// volume over a tiled floor with three bouncing balls (the first carries the
// light). Drag horizontally to orbit the camera.
//
// Fog state lives in buffer_a: velocity.xy, density.z. The pressure solve
// reads the previous frame's pressure from the other of C/D each pass, so
// one Jacobi iteration happens per frame; buffer_a subtracts the pressure
// gradient from velocity at read time (projection).

import package::shadertoy::i_channel_0;
import package::shadertoy::i_channel_1;
import package::shadertoy::i_channel_2;
import package::shadertoy::i_channel_3;
import package::shadertoy::i_frame;
import package::shadertoy::i_mouse;
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

// Advection step: how far the fog is carried along its velocity per frame.
const dissipation: f32 = 0.95; // tweak: 0..1.9
const ballRadius: f32 = 0.06; // tweak: 0.02..0.2
const fogHeigth: f32 = 0.24; // tweak: 0.05..0.6
// Raymarch slices (cost grows with the square).
const nbSlice: i32 = 24; // tweak: 4..48
const fogSlice: f32 = 0.01; // tweak: 0.002..0.05
const nbSphere: i32 = 3; // tweak: 1..8
const shadowDensity: f32 = 25.; // tweak: 0..100
const fogDensity: f32 = 20.; // tweak: 0..50
// Height of the light above the first ball (must stay above the fog).
const lightHeight: f32 = 1.; // tweak: 0.3..3
const tau: f32 = 6.28318530718; // tweak: off

// How much fog survives each frame.
const DENSITY_DECAY: f32 = 0.99; // tweak: 0.95..1
// How hard a moving ball pushes the fog.
const BALL_PUSH: f32 = 5.; // tweak: 0..20
const BALL_COLOR: vec3<f32> = vec3<f32>(1., 0., 0.); // tweak: color

fn hash12(p: vec2<f32>) -> f32 {
    var p3: vec3<f32> = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 = p3 + (dot(p3, p3.yzx + 33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn hash41(p: f32) -> vec4<f32> {
    var p4: vec4<f32> = fract(vec4<f32>(p) * vec4<f32>(0.1031, 0.103, 0.0973, 0.1099));
    p4 = p4 + (dot(p4, p4.wzxy + 33.33));
    return fract((p4.xxyz + p4.yzzw) * p4.zywx);
}

fn sphIntersect(ro: vec3<f32>, rd: vec3<f32>, ce: vec3<f32>, ra: f32) -> vec2<f32> {
    let oc: vec3<f32> = ro - ce;
    let b: f32 = dot(oc, rd);
    let c: f32 = dot(oc, oc) - ra * ra;
    var h: f32 = b * b - c;
    if (h < 0.) {
        return vec2<f32>(-1.);
    }
    h = sqrt(h);
    return vec2<f32>(-b - h, -b + h);
}

fn boxIntersection(ro: vec3<f32>, rd: vec3<f32>, rad: vec3<f32>, center: vec3<f32>, oN: ptr<function, vec3<f32>>) -> vec2<f32> {
    var ro_var = ro;
    ro_var = ro_var - (center);
    let m: vec3<f32> = 1. / rd;
    let n: vec3<f32> = m * ro_var;
    let k: vec3<f32> = abs(m) * rad;
    let t1: vec3<f32> = -n - k;
    let t2: vec3<f32> = -n + k;
    let tN: f32 = max(max(t1.x, t1.y), t1.z);
    let tF: f32 = min(min(t2.x, t2.y), t2.z);
    if (tN > tF || tF < 0.) {
        return vec2<f32>(-1.);
    }
    (*oN) = -sign(rd) * step(t1.yzx, t1.xyz) * step(t1.zxy, t1.xyz);
    return vec2<f32>(tN, tF);
}

fn spherePosition(id: i32, frame: i32) -> vec2<f32> {
    let offset: vec4<f32> = hash41(f32(id)) * tau;
    let fframe: f32 = f32(frame);
    return vec2<f32>(cos(offset.x + fframe * 0.015) + cos(offset.y + fframe * 0.02), cos(offset.z + fframe * 0.017) + cos(offset.w + fframe * 0.022)) * vec2<f32>(1., 0.5) * 0.9;
}

fn dist2(v: vec3<f32>) -> f32 {
    return dot(v, v);
}

fn floorIntersect(ro: vec3<f32>, rd: vec3<f32>, floorHeight: f32, t: ptr<function, f32>) -> bool {
    var ro_var = ro;
    ro_var.y = ro_var.y - (floorHeight);
    if (rd.y < -0.01) {
        (*t) = ro_var.y / -rd.y;
        return true;
    }
    return false;
}

// Bilinear sample at normalized uv (the reference's sample_texture, with the
// storage textures replaced by channel handles; channels are unfilterable
// Rgba32Float, so filter by hand).
fn sample_bilinear(ch: texture_2d<f32>, U01: vec2<f32>) -> vec4<f32> {
    let U = U01 * i_resolution().xy;
    let f = vec2<i32>(floor(U));
    let c = vec2<i32>(ceil(U));
    let fr = fract(U);

    let interpolated_2d = (
        (1. - fr.x) * (1. - fr.y) * textureLoad(ch, f, 0)
        + (1. - fr.x) * fr.y * textureLoad(ch, vec2<i32>(f.x, c.y), 0)
        + fr.x * fr.y * textureLoad(ch, c, 0)
        + fr.x * (1. - fr.y) * textureLoad(ch, vec2<i32>(c.x, f.y), 0)
    );
    return interpolated_2d;
}

// ---- buffer_a: fog velocity/density simulation ----

fn noise(p_in: vec3<f32>) -> f32 {
    var p = p_in;
    let ip: vec3<f32> = floor(p);
    p = p - (ip);
    let s: vec3<f32> = vec3<f32>(7., 157., 113.);
    var h: vec4<f32> = vec4<f32>(0., s.yz, s.y + s.z) + dot(ip, s);
    p = p * p * (3. - 2. * p);
    h = mix(fract(sin(h) * 43758.5), fract(sin(h + s.x) * 43758.5), p.x);
    var hxy = h.xy;
    hxy = mix(h.xz, h.yw, p.y);
    h.x = hxy.x;
    h.y = hxy.y;
    return mix(h.x, h.y, p.z);
}

fn fbm(p_in: vec3<f32>, octaveNum: i32) -> vec2<f32> {
    var p = p_in;
    var acc: vec2<f32> = vec2<f32>(0.);
    var amp: f32 = 0.5;
    let shift: vec3<f32> = vec3<f32>(100.);

    for (var i: i32 = 0; i < octaveNum; i = i + 1) {
        acc = acc + (vec2<f32>(noise(p), noise(p + vec3<f32>(0., 0., 10.))) * amp);
        p = p * 2. + shift;
        amp = amp * (0.5);
    }
    return acc;
}

// Velocity/density advected sample with the pressure gradient subtracted
// (pressure lives in buffer_d.x).
fn sampleMinusGradient(coord: vec2<f32>) -> vec3<f32> {
    let R = i_resolution().xy;
    let veld: vec3<f32> = sample_bilinear(i_channel_0, (coord / R)).xyz;
    let left: f32 = sample_bilinear(i_channel_3, ((coord + vec2<f32>(-1., 0.)) / R)).x;
    let right: f32 = sample_bilinear(i_channel_3, ((coord + vec2<f32>(1., 0.)) / R)).x;
    let bottom: f32 = sample_bilinear(i_channel_3, ((coord + vec2<f32>(0., -1.)) / R)).x;
    let top: f32 = sample_bilinear(i_channel_3, ((coord + vec2<f32>(0., 1.)) / R)).x;
    let grad: vec2<f32> = vec2<f32>(right - left, top - bottom) * 0.5;
    return vec3<f32>(veld.xy - grad, veld.z);
}

fn vignetteSim(color: vec3<f32>, q: vec2<f32>, v: f32) -> vec3<f32> {
    return color * (mix(1., pow(16. * q.x * q.y * (1. - q.x) * (1. - q.y), v), 0.02));
}

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    let fragCoord = frag_coord;

    var velocity: vec2<f32> = sampleMinusGradient(fragCoord).xy;
    var veld: vec3<f32> = sampleMinusGradient(fragCoord - dissipation * velocity).xyz;
    var density: f32 = veld.z;
    velocity = veld.xy;
    let uv: vec2<f32> = (2. * fragCoord - R) / R.y;
    let detailNoise: vec2<f32> = fbm(vec3<f32>(uv * 40., i_time() * 0.5 + 30.), 7) - 0.5;
    velocity = velocity + (detailNoise * 0.2);
    density = density + (length(detailNoise) * 0.01);
    let injectionNoise: vec2<f32> = fbm(vec3<f32>(uv * 1.5, i_time() * 0.1 + 30.), 7) - 0.5;
    velocity = velocity + (injectionNoise * 0.1);
    density = density + (max(length(injectionNoise) * 0.04, 0.));
    let influenceRadius: f32 = ballRadius * 2.;
    let frame: i32 = i32(i_frame());

    for (var i: i32 = 0; i < nbSphere; i = i + 1) {
        let p: vec2<f32> = spherePosition(i, frame);
        let dist: f32 = distance(uv, p);
        if (dist < influenceRadius) {
            let op: vec2<f32> = spherePosition(i, frame + 1);
            let ballVelocity: vec2<f32> = p - op;
            density = density - ((influenceRadius - dist) / influenceRadius * 0.15);
            density = max(0., density);
            velocity = velocity - (ballVelocity * BALL_PUSH);
        }
    }

    density = min(1., density);
    density = density * (DENSITY_DECAY);
    var state = vec3<f32>(velocity, density);
    state = vignetteSim(state, fragCoord / R, 1.);
    return vec4<f32>(state, 1.);
}

// ---- buffer_b: divergence of the fog velocity ----

fn buffer_b(frag_coord: vec2<f32>) -> vec4<f32> {
    let icoord: vec2<i32> = vec2<i32>(frag_coord);
    let vel_x_left: f32 = textureLoad(i_channel_0, icoord + vec2<i32>(-1, 0), 0).x;
    let vel_x_right: f32 = textureLoad(i_channel_0, icoord + vec2<i32>(1, 0), 0).x;
    let vel_y_bottom: f32 = textureLoad(i_channel_0, icoord + vec2<i32>(0, -1), 0).y;
    let vel_y_top: f32 = textureLoad(i_channel_0, icoord + vec2<i32>(0, 1), 0).y;
    let divergence: f32 = (vel_x_right - vel_x_left + vel_y_top - vel_y_bottom) * 0.5;
    return vec4<f32>(divergence, vec3<f32>(1.));
}

// ---- buffer_c / buffer_d: Jacobi pressure solve ----

// The reference's 121-tap getPre kernel is exactly the rotated binomial
// weight C(10,(10+dx+dy)/2) * C(10,(10+dx-dy)/2) over the diamond
// |dx| + |dy| <= 10 with dx+dy even (verified tap-by-tap), so it is written
// as a loop over the binomial row instead of 121 explicit taps.
const C10: array<f32, 11> = array<f32, 11>(1., 10., 45., 120., 210., 252., 210., 120., 45., 10., 1.);

fn blurPressure(ch: texture_2d<f32>, loc: vec2<i32>) -> f32 {
    var p: f32 = 0.;
    for (var dx: i32 = -10; dx <= 10; dx = dx + 1) {
        for (var dy: i32 = -10; dy <= 10; dy = dy + 1) {
            if (abs(dx) + abs(dy) > 10 || (dx + dy) % 2 != 0) {
                continue;
            }
            let w = C10[(10 + dx + dy) / 2] * C10[(10 + dx - dy) / 2];
            p = p + (w * textureLoad(ch, loc + vec2<i32>(dx, dy), 0).x);
        }
    }
    return p / 1048576.;
}

fn div_at(ch: texture_2d<f32>, loc: vec2<i32>, x: i32, y: i32) -> f32 {
    return textureLoad(ch, loc + vec2<i32>(x, y), 0).x;
}

// 181-tap smoothing of the divergence field (radius-9 diamond), transcribed
// from the reference's explicit tap list.
fn getDiv(ch: texture_2d<f32>, loc: vec2<i32>) -> f32 {
    var p: f32 = 0.;
		p = p + (1 * div_at(ch, loc, -9, 0));
		p = p + (9 * div_at(ch, loc, -8, -1));
		p = p + (4 * div_at(ch, loc, -8, 0));
		p = p + (9 * div_at(ch, loc, -8, 1));
		p = p + (36 * div_at(ch, loc, -7, -2));
		p = p + (32 * div_at(ch, loc, -7, -1));
		p = p + (97 * div_at(ch, loc, -7, 0));
		p = p + (32 * div_at(ch, loc, -7, 1));
		p = p + (36 * div_at(ch, loc, -7, 2));
		p = p + (84 * div_at(ch, loc, -6, -3));
		p = p + (112 * div_at(ch, loc, -6, -2));
		p = p + (436 * div_at(ch, loc, -6, -1));
		p = p + (320 * div_at(ch, loc, -6, 0));
		p = p + (436 * div_at(ch, loc, -6, 1));
		p = p + (112 * div_at(ch, loc, -6, 2));
		p = p + (84 * div_at(ch, loc, -6, 3));
		p = p + (126 * div_at(ch, loc, -5, -4));
		p = p + (224 * div_at(ch, loc, -5, -3));
		p = p + (1092 * div_at(ch, loc, -5, -2));
		p = p + (1280 * div_at(ch, loc, -5, -1));
		p = p + (2336 * div_at(ch, loc, -5, 0));
		p = p + (1280 * div_at(ch, loc, -5, 1));
		p = p + (1092 * div_at(ch, loc, -5, 2));
		p = p + (224 * div_at(ch, loc, -5, 3));
		p = p + (126 * div_at(ch, loc, -5, 4));
		p = p + (126 * div_at(ch, loc, -4, -5));
		p = p + (280 * div_at(ch, loc, -4, -4));
		p = p + (1694 * div_at(ch, loc, -4, -3));
		p = p + (2752 * div_at(ch, loc, -4, -2));
		p = p + (6656 * div_at(ch, loc, -4, -1));
		p = p + (6464 * div_at(ch, loc, -4, 0));
		p = p + (6656 * div_at(ch, loc, -4, 1));
		p = p + (2752 * div_at(ch, loc, -4, 2));
		p = p + (1694 * div_at(ch, loc, -4, 3));
		p = p + (280 * div_at(ch, loc, -4, 4));
		p = p + (126 * div_at(ch, loc, -4, 5));
		p = p + (84 * div_at(ch, loc, -3, -6));
		p = p + (224 * div_at(ch, loc, -3, -5));
		p = p + (1694 * div_at(ch, loc, -3, -4));
		p = p + (3520 * div_at(ch, loc, -3, -3));
		p = p + (11016 * div_at(ch, loc, -3, -2));
		p = p + (16128 * div_at(ch, loc, -3, -1));
		p = p + (24608 * div_at(ch, loc, -3, 0));
		p = p + (16128 * div_at(ch, loc, -3, 1));
		p = p + (11016 * div_at(ch, loc, -3, 2));
		p = p + (3520 * div_at(ch, loc, -3, 3));
		p = p + (1694 * div_at(ch, loc, -3, 4));
		p = p + (224 * div_at(ch, loc, -3, 5));
		p = p + (84 * div_at(ch, loc, -3, 6));
		p = p + (36 * div_at(ch, loc, -2, -7));
		p = p + (112 * div_at(ch, loc, -2, -6));
		p = p + (1092 * div_at(ch, loc, -2, -5));
		p = p + (2752 * div_at(ch, loc, -2, -4));
		p = p + (11016 * div_at(ch, loc, -2, -3));
		p = p + (21664 * div_at(ch, loc, -2, -2));
		p = p + (47432 * div_at(ch, loc, -2, -1));
		p = p + (59712 * div_at(ch, loc, -2, 0));
		p = p + (47432 * div_at(ch, loc, -2, 1));
		p = p + (21664 * div_at(ch, loc, -2, 2));
		p = p + (11016 * div_at(ch, loc, -2, 3));
		p = p + (2752 * div_at(ch, loc, -2, 4));
		p = p + (1092 * div_at(ch, loc, -2, 5));
		p = p + (112 * div_at(ch, loc, -2, 6));
		p = p + (36 * div_at(ch, loc, -2, 7));
		p = p + (9 * div_at(ch, loc, -1, -8));
		p = p + (32 * div_at(ch, loc, -1, -7));
		p = p + (436 * div_at(ch, loc, -1, -6));
		p = p + (1280 * div_at(ch, loc, -1, -5));
		p = p + (6656 * div_at(ch, loc, -1, -4));
		p = p + (16128 * div_at(ch, loc, -1, -3));
		p = p + (47432 * div_at(ch, loc, -1, -2));
		p = p + (92224 * div_at(ch, loc, -1, -1));
		p = p + (163476 * div_at(ch, loc, -1, 0));
		p = p + (92224 * div_at(ch, loc, -1, 1));
		p = p + (47432 * div_at(ch, loc, -1, 2));
		p = p + (16128 * div_at(ch, loc, -1, 3));
		p = p + (6656 * div_at(ch, loc, -1, 4));
		p = p + (1280 * div_at(ch, loc, -1, 5));
		p = p + (436 * div_at(ch, loc, -1, 6));
		p = p + (32 * div_at(ch, loc, -1, 7));
		p = p + (9 * div_at(ch, loc, -1, 8));
		p = p + (1 * div_at(ch, loc, 0, -9));
		p = p + (4 * div_at(ch, loc, 0, -8));
		p = p + (97 * div_at(ch, loc, 0, -7));
		p = p + (320 * div_at(ch, loc, 0, -6));
		p = p + (2336 * div_at(ch, loc, 0, -5));
		p = p + (6464 * div_at(ch, loc, 0, -4));
		p = p + (24608 * div_at(ch, loc, 0, -3));
		p = p + (59712 * div_at(ch, loc, 0, -2));
		p = p + (163476 * div_at(ch, loc, 0, -1));
		p = p + (409744 * div_at(ch, loc, 0, 0));
		p = p + (163476 * div_at(ch, loc, 0, 1));
		p = p + (59712 * div_at(ch, loc, 0, 2));
		p = p + (24608 * div_at(ch, loc, 0, 3));
		p = p + (6464 * div_at(ch, loc, 0, 4));
		p = p + (2336 * div_at(ch, loc, 0, 5));
		p = p + (320 * div_at(ch, loc, 0, 6));
		p = p + (97 * div_at(ch, loc, 0, 7));
		p = p + (4 * div_at(ch, loc, 0, 8));
		p = p + (1 * div_at(ch, loc, 0, 9));
		p = p + (9 * div_at(ch, loc, 1, -8));
		p = p + (32 * div_at(ch, loc, 1, -7));
		p = p + (436 * div_at(ch, loc, 1, -6));
		p = p + (1280 * div_at(ch, loc, 1, -5));
		p = p + (6656 * div_at(ch, loc, 1, -4));
		p = p + (16128 * div_at(ch, loc, 1, -3));
		p = p + (47432 * div_at(ch, loc, 1, -2));
		p = p + (92224 * div_at(ch, loc, 1, -1));
		p = p + (163476 * div_at(ch, loc, 1, 0));
		p = p + (92224 * div_at(ch, loc, 1, 1));
		p = p + (47432 * div_at(ch, loc, 1, 2));
		p = p + (16128 * div_at(ch, loc, 1, 3));
		p = p + (6656 * div_at(ch, loc, 1, 4));
		p = p + (1280 * div_at(ch, loc, 1, 5));
		p = p + (436 * div_at(ch, loc, 1, 6));
		p = p + (32 * div_at(ch, loc, 1, 7));
		p = p + (9 * div_at(ch, loc, 1, 8));
		p = p + (36 * div_at(ch, loc, 2, -7));
		p = p + (112 * div_at(ch, loc, 2, -6));
		p = p + (1092 * div_at(ch, loc, 2, -5));
		p = p + (2752 * div_at(ch, loc, 2, -4));
		p = p + (11016 * div_at(ch, loc, 2, -3));
		p = p + (21664 * div_at(ch, loc, 2, -2));
		p = p + (47432 * div_at(ch, loc, 2, -1));
		p = p + (59712 * div_at(ch, loc, 2, 0));
		p = p + (47432 * div_at(ch, loc, 2, 1));
		p = p + (21664 * div_at(ch, loc, 2, 2));
		p = p + (11016 * div_at(ch, loc, 2, 3));
		p = p + (2752 * div_at(ch, loc, 2, 4));
		p = p + (1092 * div_at(ch, loc, 2, 5));
		p = p + (112 * div_at(ch, loc, 2, 6));
		p = p + (36 * div_at(ch, loc, 2, 7));
		p = p + (84 * div_at(ch, loc, 3, -6));
		p = p + (224 * div_at(ch, loc, 3, -5));
		p = p + (1694 * div_at(ch, loc, 3, -4));
		p = p + (3520 * div_at(ch, loc, 3, -3));
		p = p + (11016 * div_at(ch, loc, 3, -2));
		p = p + (16128 * div_at(ch, loc, 3, -1));
		p = p + (24608 * div_at(ch, loc, 3, 0));
		p = p + (16128 * div_at(ch, loc, 3, 1));
		p = p + (11016 * div_at(ch, loc, 3, 2));
		p = p + (3520 * div_at(ch, loc, 3, 3));
		p = p + (1694 * div_at(ch, loc, 3, 4));
		p = p + (224 * div_at(ch, loc, 3, 5));
		p = p + (84 * div_at(ch, loc, 3, 6));
		p = p + (126 * div_at(ch, loc, 4, -5));
		p = p + (280 * div_at(ch, loc, 4, -4));
		p = p + (1694 * div_at(ch, loc, 4, -3));
		p = p + (2752 * div_at(ch, loc, 4, -2));
		p = p + (6656 * div_at(ch, loc, 4, -1));
		p = p + (6464 * div_at(ch, loc, 4, 0));
		p = p + (6656 * div_at(ch, loc, 4, 1));
		p = p + (2752 * div_at(ch, loc, 4, 2));
		p = p + (1694 * div_at(ch, loc, 4, 3));
		p = p + (280 * div_at(ch, loc, 4, 4));
		p = p + (126 * div_at(ch, loc, 4, 5));
		p = p + (126 * div_at(ch, loc, 5, -4));
		p = p + (224 * div_at(ch, loc, 5, -3));
		p = p + (1092 * div_at(ch, loc, 5, -2));
		p = p + (1280 * div_at(ch, loc, 5, -1));
		p = p + (2336 * div_at(ch, loc, 5, 0));
		p = p + (1280 * div_at(ch, loc, 5, 1));
		p = p + (1092 * div_at(ch, loc, 5, 2));
		p = p + (224 * div_at(ch, loc, 5, 3));
		p = p + (126 * div_at(ch, loc, 5, 4));
		p = p + (84 * div_at(ch, loc, 6, -3));
		p = p + (112 * div_at(ch, loc, 6, -2));
		p = p + (436 * div_at(ch, loc, 6, -1));
		p = p + (320 * div_at(ch, loc, 6, 0));
		p = p + (436 * div_at(ch, loc, 6, 1));
		p = p + (112 * div_at(ch, loc, 6, 2));
		p = p + (84 * div_at(ch, loc, 6, 3));
		p = p + (36 * div_at(ch, loc, 7, -2));
		p = p + (32 * div_at(ch, loc, 7, -1));
		p = p + (97 * div_at(ch, loc, 7, 0));
		p = p + (32 * div_at(ch, loc, 7, 1));
		p = p + (36 * div_at(ch, loc, 7, 2));
		p = p + (9 * div_at(ch, loc, 8, -1));
		p = p + (4 * div_at(ch, loc, 8, 0));
		p = p + (9 * div_at(ch, loc, 8, 1));
		p = p + (1 * div_at(ch, loc, 9, 0));
    return p / 1048576.;
}

// buffer_c: smooth the divergence, subtract it from the smoothed previous
// pressure (buffer_d.x); keep both in .x/.y for buffer_d.
fn buffer_c(frag_coord: vec2<f32>) -> vec4<f32> {
    let loc: vec2<i32> = vec2<i32>(frag_coord);
    let divergence: f32 = getDiv(i_channel_1, loc);
    let p: f32 = blurPressure(i_channel_3, loc) - divergence;
    return vec4<f32>(p, divergence, vec2<f32>(1.));
}

// buffer_d: one more Jacobi step reading this frame's pressure/divergence
// pair from buffer_c.
fn buffer_d(frag_coord: vec2<f32>) -> vec4<f32> {
    let loc: vec2<i32> = vec2<i32>(frag_coord);
    let p: f32 = blurPressure(i_channel_2, loc) - textureLoad(i_channel_2, loc, 0).y;
    return vec4<f32>(p, vec3<f32>(1.));
}

// ---- image: raymarched fog over the tiled floor ----

fn sceneIntersection(ro: vec3<f32>, rd: vec3<f32>, inter: ptr<function, vec3<f32>>, normal: ptr<function, vec3<f32>>, color: ptr<function, vec3<f32>>, dist: f32, lightPos: ptr<function, vec3<f32>>) -> f32 {
    var mint: f32 = dist;
    (*inter) = vec3<f32>(0.);
    (*normal) = vec3<f32>(0.);
    (*color) = vec3<f32>(0.);

    for (var i: i32 = 0; i < nbSphere; i = i + 1) {
        let p2d: vec2<f32> = spherePosition(i, i32(i_frame()));
        let pos: vec3<f32> = vec3<f32>(p2d.x, ballRadius, p2d.y);
        var ballColor: vec3<f32> = BALL_COLOR;
        if (i == 0) {
            ballColor = vec3<f32>(1.);
            (*lightPos) = pos + vec3<f32>(0., lightHeight, 0.);
        }
        let t: f32 = sphIntersect(ro, rd, pos, ballRadius).x;
        if (t > 0. && t < mint) {
            mint = t;
            (*inter) = ro + mint * rd;
            (*normal) = normalize((*inter) - pos);
            (*color) = ballColor;
        }
    }

    let aspecRatio: f32 = i_resolution().x / i_resolution().y;
    var boxNormal: vec3<f32> = vec3<f32>(0.);
    let t: f32 = boxIntersection(ro, rd, vec3<f32>(aspecRatio, 0.1, 1.), vec3<f32>(0., -0.1, 0.), &boxNormal).x;
    if (t > 0. && t < mint) {
        mint = t;
        (*inter) = ro + mint * rd;
        (*normal) = boxNormal;
        let tileId: vec2<i32> = vec2<i32>(vec2<f32>((*inter).x, (*inter).z) * 3. + 100.);
        if (((tileId.x & 1) ^ (tileId.y & 1)) == 0) { (*color) = vec3<f32>(0.3); } else { (*color) = vec3<f32>(0.15); };
    }
    return mint;
}

fn sampleFog(pos: vec3<f32>) -> f32 {
    let R = i_resolution().xy;
    var uv: vec2<f32> = pos.xz;
    uv.x = uv.x * (R.y / R.x);
    uv = uv * 0.5 + 0.5;
    if (max(uv.x, uv.y) > 1. || min(uv.x, uv.y) < 0.) {
        return 0.;
    }
    return sample_bilinear(i_channel_0, uv).z;
}

fn Render(ro: vec3<f32>, rd: vec3<f32>, dist: f32, fudge: f32) -> vec3<f32> {
    var inter: vec3<f32> = vec3<f32>(0.);
    var normal: vec3<f32> = vec3<f32>(0.);
    var baseColor: vec3<f32> = vec3<f32>(0.);
    var lightPos: vec3<f32> = vec3<f32>(0.);
    let mint: f32 = sceneIntersection(ro, rd, &inter, &normal, &baseColor, dist, &lightPos);
    var color: vec3<f32> = vec3<f32>(0.);
    if (mint < dist) {
        let lightDir: vec3<f32> = normalize(lightPos - inter);
        let lightDist2: f32 = dist2(lightPos - inter);
        let shadowStep: vec3<f32> = fogHeigth / f32(nbSlice) * lightDir / lightDir.y;
        var shadowDist: f32 = 0.;

        for (var i: i32 = 0; i < nbSlice; i = i + 1) {
            let shadowPos: vec3<f32> = inter + shadowStep * f32(i);
            let v: f32 = sampleFog(shadowPos) * fogHeigth;
            shadowDist = shadowDist + (min(max(0., v - shadowPos.y), fogSlice) * length(shadowStep) / fogSlice);
        }

        let shadowFactor: f32 = exp(-shadowDist * shadowDensity * 0.25);
        color = baseColor * (max(0., dot(normal, lightDir) * shadowFactor) + 0.2) / lightDist2;
    }
    var t: f32 = 0.;
    if (floorIntersect(ro, rd, fogHeigth, &t)) {
        var curPos: vec3<f32> = ro + rd * t;
        let fogStep: vec3<f32> = fogHeigth / f32(nbSlice) * rd / abs(rd.y);
        curPos = curPos + (fudge * fogStep);
        let stepLen: f32 = length(fogStep);
        var curDensity: f32 = 0.;
        var transmittance: f32 = 1.;
        var lightEnergy: f32 = 0.;

        for (var i: i32 = 0; i < nbSlice; i = i + 1) {
            if (dot(curPos - ro, rd) > mint) {
                break;
            }
            let curHeigth: f32 = sampleFog(curPos) * fogHeigth;
            let curSample: f32 = min(max(0., curHeigth - curPos.y), fogSlice) * stepLen / fogSlice;
            if (curSample > 0.001) {
                let lightDir: vec3<f32> = normalize(lightPos - curPos);
                let shadowStep: vec3<f32> = fogHeigth / f32(nbSlice) * lightDir / lightDir.y;
                let lightDist2: f32 = dist2(lightPos - curPos);
                var shadowPos: vec3<f32> = curPos + shadowStep * fudge;
                var shadowDist: f32 = 0.;

                for (var j: i32 = 0; j < nbSlice; j = j + 1) {
                    shadowPos = shadowPos + (shadowStep);
                    if (shadowPos.y > fogHeigth) {
                        break;
                    }
                    let curHeight: f32 = sampleFog(shadowPos) * fogHeigth;
                    shadowDist = shadowDist + (min(max(0., curHeight - shadowPos.y), fogSlice) * length(shadowStep) / fogSlice);
                }

                let shadowFactor: f32 = exp(-shadowDist * shadowDensity) / lightDist2;
                curDensity = curSample * fogDensity;
                let absorbedlight: f32 = shadowFactor * (1. * curDensity);
                lightEnergy = lightEnergy + (absorbedlight * transmittance);
                transmittance = transmittance * (1. - curDensity);
            }
            curPos = curPos + (fogStep);
        }

        color = mix(color, vec3<f32>(lightEnergy), 1. - transmittance);
    }
    return color;
}

fn vignetteImg(color: vec3<f32>, q: vec2<f32>, v: f32) -> vec3<f32> {
    return color * (0.3 + 0.8 * pow(16. * q.x * q.y * (1. - q.x) * (1. - q.y), v));
}

fn setCamera(ro: vec3<f32>, ta: vec3<f32>) -> mat3x3<f32> {
    let cw: vec3<f32> = normalize(ta - ro);
    let up: vec3<f32> = vec3<f32>(0., 1., 0.);
    let cu: vec3<f32> = normalize(cross(cw, up));
    let cv: vec3<f32> = normalize(cross(cu, cw));
    return mat3x3<f32>(cu, cv, cw);
}

fn radians(degrees: f32) -> f32 {
    return degrees * (3.1416 / 180.);
}

fn toLinear(sRGB: vec4<f32>) -> vec4<f32> {
    let cutoff = vec4<f32>(sRGB < vec4<f32>(0.04045));
    let higher = pow((sRGB + vec4<f32>(0.055)) / vec4<f32>(1.055), vec4<f32>(2.4));
    let lower = sRGB / vec4<f32>(12.92);
    return mix(higher, lower, cutoff);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    let p: vec2<f32> = (-R + 2. * frag_coord) / R.y;
    let m = i_mouse();
    let theta: f32 = radians(360.) * (m.x / R.x - 0.5) - radians(90.);
    let phi: f32 = -radians(30.);
    let ro: vec3<f32> = 2. * vec3<f32>(sin(phi) * cos(theta), cos(phi), sin(phi) * sin(theta));
    let ca: mat3x3<f32> = setCamera(ro, vec3<f32>(0.));
    let rd: vec3<f32> = ca * normalize(vec3<f32>(p, 1.5));
    let col: vec3<f32> = Render(ro, rd, 6., hash12(frag_coord + i_time()));
    let tot: vec3<f32> = vignetteImg(col, frag_coord / R, 0.6);
    return toLinear(vec4<f32>(sqrt(tot), 1.));
}
