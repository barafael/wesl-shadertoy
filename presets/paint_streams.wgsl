// Paint Streams — https://www.shadertoy.com/view/WtfyDj by michael0884
// (CC BY-NC-SA 3.0), ported from eliotbo/bevy_shadertoy_wgsl.
//
// A 2D "Reintegration Tracking" SPH fluid
// (https://michaelmoroz.github.io/Reintegration-Tracking/). Each texel of
// buffer_a / buffer_b stores one particle: position (as an offset from the
// texel) and velocity are packed 16 fixed-point bits at a time into single
// f32 channels — which is why the buffer targets are Rgba32Float. buffer_a
// resamples (reintegrates) the particle cloud from buffer_b; buffer_b applies
// pressure, boundary and mouse forces and feeds two paint jets near the top
// of the screen; buffer_c splats the particles into a density + dye field;
// the image pass shades that field as two streams of paint (M.y is the dye:
// orange vs blue).
//
// The reference's INIT pass (seeding a low-mass particle in every texel) is
// dropped: the textures start zeroed and the sim bootstraps from the jets.

import package::shadertoy::i_channel_0;
import package::shadertoy::i_channel_1;
import package::shadertoy::i_channel_2;
import package::shadertoy::i_mouse;
import package::shadertoy::i_resolution;

const PI: f32 = 3.14159265; // tweak: off
// Time step. Particles move up to dt texels per frame and are gathered from
// a 5x5 neighbourhood, so much more than ~3 tears the fluid apart.
const dt: f32 = 2.5; // tweak: 0.5..3
// Width of the wall's contact zone (sim) and dark rim (image).
const border_h: f32 = 5.; // tweak: 1..10
// Finite-difference step for the wall normal.
const h: f32 = 1.; // tweak: 0.5..2
// Rest density: what the jets inject and what pressure pushes toward.
const fluid_rho: f32 = 0.5; // tweak: 0.2..1

const GRAVITY: f32 = 0.0004; // tweak: 0..0.002
const MOUSE_FORCE: f32 = 0.001; // tweak: 0..0.005
// The two paint jets: radius (px) and launch speed (speed is capped at 1).
const JET_RADIUS: f32 = 20.; // tweak: 5..40
const JET_SPEED: f32 = 0.5; // tweak: 0..1
const PAINT_COLOR_A: vec3<f32> = vec3<f32>(1., 0.5, 0.); // tweak: color
const PAINT_COLOR_B: vec3<f32> = vec3<f32>(0.1, 0.4, 1.); // tweak: color

fn Dir(ang: f32) -> vec2<f32> {
    return vec2<f32>(cos(ang), sin(ang));
}

fn sdBox(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.))) + min(max(d.x, d.y), 0.);
}

// Pressure as a function of density (surface tension term keeps it positive
// below fluid_rho).
fn Pf(rho: vec2<f32>) -> f32 {
    let GF: f32 = 1.;
    return mix(0.5 * rho.x, 0.04 * rho.x * (rho.x / fluid_rho - 1.), GF);
}

fn G(x: vec2<f32>) -> f32 {
    return exp(-dot(x, x));
}

// Two [-1, 1] values packed as 16-bit fixed point into one u32, stored as an
// exact f32 bit pattern in a 32-bit channel.
fn decode2(input: u32, place: u32, precis: u32) -> f32 {
    let value_u32 = input >> (place - precis);
    var mask: u32 = 4294967295u;
    if (precis < 32u) {
        mask = (1u << precis) - 1u;
    }
    let masked_value_u32 = value_u32 & mask;
    let max_val = 1u << (precis - 1u);
    return f32(masked_value_u32) / f32(max_val);
}

fn decode1uToVec2(q: f32) -> vec2<f32> {
    let uq = u32(q);
    let x = decode2(uq, 32u, 16u);
    let y = decode2(uq, 16u, 16u);
    return vec2<f32>(x, y) * 2. - 1.;
}

fn encode2(value: f32, input2: u32, place: u32, precis: u32) -> u32 {
    let value_f32_normalized = value * f32(1u << (precis - 1u));
    let delta_bits = u32(place - precis);
    let value_u32 = u32(value_f32_normalized) << delta_bits;
    var mask: u32 = 0u;
    if (precis < 32u) {
        mask = 4294967295u - (((1u << precis) - 1u) << (place - precis));
    }
    let input = (input2 & mask) | value_u32;
    return input;
}

fn encodeVec2To1u(value: vec2<f32>) -> u32 {
    var input: u32 = 0u;
    let x = clamp(0.5 * value.x + 0.5, 0., 1.);
    let y = clamp(0.5 * value.y + 0.5, 0., 1.);
    input = encode2(x, input, 32u, 16u);
    input = encode2(y, input, 16u, 16u);
    return input;
}

struct particle {
    X: vec2<f32>,
    V: vec2<f32>,
    M: vec2<f32>,
}

fn getParticle(data: vec4<f32>, pos: vec2<f32>) -> particle {
    var P = particle(vec2<f32>(0.), vec2<f32>(0.), vec2<f32>(0.));
    P.X = decode1uToVec2(data.x) + pos;
    P.V = decode1uToVec2(data.y);
    P.M = data.zw;
    return P;
}

fn saveParticle(PIn: particle, pos: vec2<f32>) -> vec4<f32> {
    var P = PIn;
    P.X = clamp(P.X - pos, vec2<f32>(-0.5), vec2<f32>(0.5));
    return vec4<f32>(f32(encodeVec2To1u(P.X)), f32(encodeVec2To1u(P.V)), P.M);
}

// Overlap of the particle's (K-wide) support with the texel p: xy = the
// redistributed position, z = the mass fraction.
fn distribution(x: vec2<f32>, p: vec2<f32>, K: f32) -> vec3<f32> {
    let omin = clamp(x - K * 0.5, p - 0.5, p + 0.5);
    let omax = clamp(x + K * 0.5, p - 0.5, p + 0.5);
    return vec3<f32>(0.5 * (omin + omax), (omax.x - omin.x) * (omax.y - omin.y) / (K * K));
}

// Signed distance to the fluid domain: the screen rect is the tank, the
// "drain" box keeps the lower area fluid (it overrides the small bar).
fn border(p: vec2<f32>, R2: vec2<f32>) -> f32 {
    let bound = -sdBox(p - R2 * 0.5, R2 * vec2<f32>(0.5, 0.5));
    let bar = sdBox(p - R2 * vec2<f32>(0.5, 0.6), R2 * vec2<f32>(0.05, 0.01));
    let drain = -sdBox(p - R2 * vec2<f32>(0.5, 0.7), R2 * vec2<f32>(1.5, 0.5));
    return max(drain, min(bound, bar));
}

// Border normal + distance at p (z is the SDF, nudged off zero).
fn bN(p: vec2<f32>, R2: vec2<f32>) -> vec3<f32> {
    let dx = vec3<f32>(-h, 0., h);
    let idx = vec4<f32>(-1. / h, 0., 1. / h, 0.25);
    let r = idx.zyw * border(p + dx.zy, R2) + idx.xyw * border(p + dx.xy, R2) + idx.yzw * border(p + dx.yz, R2) + idx.yxw * border(p + dx.yx, R2);
    return vec3<f32>(normalize(r.xy), r.z + 1.0e-4);
}

// Reintegration Tracking: resample buffer_b's particles onto the grid.
fn Reintegration(pos: vec2<f32>) -> particle {
    var P = particle(vec2<f32>(0.), vec2<f32>(0.), vec2<f32>(0.));
    for (var i: i32 = -2; i <= 2; i = i + 1) {
        for (var j: i32 = -2; j <= 2; j = j + 1) {
            let tpos = pos + vec2<f32>(f32(i), f32(j));
            let data = textureLoad(i_channel_1, vec2<i32>(tpos), 0);
            var P0 = getParticle(data, tpos);
            P0.X = P0.X + (P0.V * dt); // integrate position
            let difR = 0.9 + 0.21 * smoothstep(fluid_rho * 0., fluid_rho * 0.333, P0.M.x);
            let D = distribution(P0.X, pos, difR);
            let m = P0.M.x * D.z;
            P.X = P.X + (D.xy * m);
            P.V = P.V + (P0.V * m);
            P.M.y = P.M.y + (P0.M.y * m);
            P.M.x = P.M.x + m;
        }
    }
    if (P.M.x > 0.0000001) {
        P.X = P.X / P.M.x;
        P.V = P.V / P.M.x;
        P.M.y = P.M.y / P.M.x;
    }
    return P;
}

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let pos = frag_coord;
    let P = Reintegration(pos);
    return saveParticle(P, pos);
}

// Pressure + gravity + mouse forces, boundary reflection, speed clamp.
// (The reference's viscosity term is multiplied by 0 and dropped.)
fn Simulation(PIn: particle, pos: vec2<f32>, mouse: vec4<f32>, R: vec2<f32>) -> particle {
    var P = PIn;
    var F = vec2<f32>(0.);
    for (var i: i32 = -2; i <= 2; i = i + 1) {
        for (var j: i32 = -2; j <= 2; j = j + 1) {
            let tpos = pos + vec2<f32>(f32(i), f32(j));
            let data = textureLoad(i_channel_0, vec2<i32>(tpos), 0);
            let P0 = getParticle(data, tpos);
            let dx = P0.X - P.X;
            let avgP = 0.5 * P0.M.x * (Pf(P.M) + Pf(P0.M));
            F = F - (0.5 * G(dx) * avgP * dx);
        }
    }

    F = F + (P.M.x * vec2<f32>(0., -GRAVITY));

    if (mouse.z > 0.) {
        let dm = (mouse.xy - mouse.zw) / 10.;
        let d = distance(mouse.xy, P.X) / 20.;
        F = F + (MOUSE_FORCE * dm * exp(-d * d));
    }
    P.V = P.V + (F * dt / P.M.x);

    let N = bN(P.X, R);
    let vdotN = step(N.z, border_h) * dot(-N.xy, P.V);
    P.V = P.V + (0.5 * (N.xy * vdotN + N.xy * abs(vdotN)));

    if (N.z < 0.) {
        P.V = vec2<f32>(0.);
    }

    let v = length(P.V);
    var dum = 1.;
    if (v > 1.) {
        dum = v;
    }
    P.V = P.V / dum;
    return P;
}

fn buffer_b(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    let pos = frag_coord;
    let data = textureLoad(i_channel_0, vec2<i32>(pos), 0);
    var P = getParticle(data, pos);

    if (P.M.x != 0.) {
        P = Simulation(P, pos, i_mouse(), R);
    }

    // Two paint jets near the top, spraying downward; M.y is the dye.
    // (timeMult is a constant 1.0 in the reference, so the spray angles are
    // fixed: sin(0.4 * 2.0 * 1.0) = sin(0.8).)
    if (length(P.X - R * vec2<f32>(0.55, 0.9)) < JET_RADIUS) {
        P.X = pos;
        P.V = JET_SPEED * Dir(-PI * 0.75 + 0.3 * sin(0.8));
        P.M = mix(P.M, vec2<f32>(fluid_rho, 1.), 0.4);
    }

    if (length(P.X - R * vec2<f32>(0.45, 0.9)) < JET_RADIUS) {
        P.X = pos;
        P.V = JET_SPEED * Dir(PI * 0.25 - PI * 0.5 + 0.3 * sin(0.8));
        P.M = mix(P.M, vec2<f32>(fluid_rho, 0.5), 0.4);
    }

    return saveParticle(P, pos);
}

// Splat the particles into a density field: z = mass, w = dye.
fn buffer_c(frag_coord: vec2<f32>) -> vec4<f32> {
    let pos = frag_coord;
    let p = vec2<i32>(pos);
    let data = textureLoad(i_channel_0, p, 0);
    let P = getParticle(data, pos);
    var rho = vec4<f32>(0.);
    for (var i: i32 = -1; i <= 1; i = i + 1) {
        for (var j: i32 = -1; j <= 1; j = j + 1) {
            let ij = vec2<i32>(i, j);
            let d = textureLoad(i_channel_0, p + ij, 0);
            let P0 = getParticle(d, pos + vec2<f32>(ij));
            rho = rho + (vec4<f32>(P.V, P.M) * G((pos - P0.X) / 0.75));
        }
    }
    return rho;
}

// Perceptually-even mix (blend in squared space).
fn mixN(a: vec3<f32>, b: vec3<f32>, k: f32) -> vec3<f32> {
    return sqrt(mix(a * a, b * b, clamp(k, 0., 1.)));
}

fn V(p: vec2<f32>) -> vec4<f32> {
    return textureLoad(i_channel_2, vec2<i32>(p), 0);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    let pos = frag_coord;

    let bord = smoothstep(2. * border_h, border_h * 0.5, border(pos, R));
    let rho = V(pos);
    let dx = vec3<f32>(-2., 0., 2.);
    let grad = -0.5 * vec4<f32>(V(pos + dx.zy).zw - V(pos + dx.xy).zw, V(pos + dx.yz).zw - V(pos + dx.yx).zw);
    let N = pow(length(grad.xz), 0.2) * normalize(grad.xz + 0.00001);
    let specular = pow(max(dot(N, Dir(1.4)), 0.), 3.5);

    let a = pow(smoothstep(fluid_rho * 0., fluid_rho * 2., rho.z), 0.1);
    let b = exp(-1.7 * smoothstep(fluid_rho * 1., fluid_rho * 7.5, rho.z));
    let fcol = mixN(PAINT_COLOR_A, PAINT_COLOR_B, tanh(3. * (rho.w - 0.7)) * 0.5 + 0.5);

    var col = vec4<f32>(3.);
    col = vec4<f32>(mixN(col.xyz, fcol * (1.5 * b + specular * 5.), a), col.w);
    col = vec4<f32>(mixN(col.xyz, vec3<f32>(0.), bord), col.w);
    col = vec4<f32>(tanh(col.xyz), col.w);
    return col;
}
