// Particles — 16 384 sparks swirling around the middle, or around the
// pointer while you hold a button.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, simulation
// teaches: compute_a/compute_b passes, storage buffers, atomics
//
// A compute pass does what a fragment pass cannot: write wherever it
// likes. `compute_b` moves every particle and adds it to the pixel it
// lands on, `atomicAdd` keeping simultaneous hits on one pixel apart;
// `compute_a` fades the pixels first, and `main_image` shows them.
//
// State lives in storage buffers, `var<storage, read_write>` at
// `@group(1) @binding(0)` .. `@binding(3)`, which every pass can read and
// write. They start out zeroed, and are zeroed again on every compile, on
// Reset and, if one is sized by the view, on resize. A `compute_x(i: u32)`
// runs once per element of the array at `@binding(0)`; a
// `compute_x(p: vec2<u32>)` once per pixel. Passes run A to D, every frame,
// before the buffer passes and the image. (WebGPU or native builds only.)
import package::shadertoy::{i_mouse, i_resolution, i_time_delta};

struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
}

// 16 384 particles, 16 bytes each.
@group(1) @binding(0)
var<storage, read_write> particles: array<Particle, 16384>;
// A runtime-sized array gets one element per pixel: how bright it is, in
// 256ths of a hit (integers, since atomics are integers).
@group(1) @binding(1)
var<storage, read_write> glow: array<atomic<u32>>;

const PULL: f32 = 500.0; // tweak: 0..2000
const SWIRL: f32 = 320.0; // tweak: 0..1000
const DRAG: f32 = 0.5; // tweak: 0..3
const FADE: f32 = 0.9; // tweak: 0.5..0.99
const EXPOSURE: f32 = 0.35; // tweak: 0.05..2
const HOT_COLOR: vec3<f32> = vec3<f32>(1.0, 0.75, 0.35);
const COLD_COLOR: vec3<f32> = vec3<f32>(0.15, 0.3, 1.0);

fn hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

// 0..1 from a hash.
fn unit(x: u32) -> f32 {
    return f32(hash(x) & 0xffffu) / 65535.0;
}

// The pixel `p` (in pixels, origin bottom-left) as an index into `glow`.
fn cell(p: vec2<u32>) -> u32 {
    return p.y * u32(i_resolution().x) + p.x;
}

// Pass A, once per pixel: fade what the particles left last frame.
fn compute_a(p: vec2<u32>) {
    let c = cell(p);
    atomicStore(&glow[c], u32(f32(atomicLoad(&glow[c])) * FADE));
}

// Pass B, once per particle (the length of `particles`, at binding 0).
fn compute_b(i: u32) {
    let res = i_resolution().xy;
    var q = particles[i];
    // A zeroed particle has not been placed yet: scatter it over the view.
    // (This also catches the zeroing on resize, which `i_frame` does not.)
    if all(q.pos == vec2<f32>(0.0)) {
        q.pos = vec2<f32>(unit(i), unit(i + 0x9e37u)) * res;
        q.vel = vec2<f32>(0.0);
    }

    let dt = min(i_time_delta(), 1.0 / 30.0);
    let m = i_mouse();
    let centre = select(0.5 * res, m.xy, m.z > 0.0);
    let d = centre - q.pos;
    let dir = d / (length(d) + 1.0);
    // Pulled in and spun around, each particle a little differently.
    let kind = 0.5 + unit(i * 3u);
    let acc = dir * PULL + vec2<f32>(-dir.y, dir.x) * SWIRL * kind;
    q.vel = (q.vel + acc * dt) * exp(-DRAG * dt);
    q.pos += q.vel * dt;

    // Bounce off the edges.
    if q.pos.x < 0.0 || q.pos.x >= res.x {
        q.vel.x = -q.vel.x;
    }
    if q.pos.y < 0.0 || q.pos.y >= res.y {
        q.vel.y = -q.vel.y;
    }
    q.pos = clamp(q.pos, vec2<f32>(0.5), res - 0.5);
    particles[i] = q;

    atomicAdd(&glow[cell(vec2<u32>(q.pos))], 256u);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let hits = f32(atomicLoad(&glow[cell(vec2<u32>(frag_coord))])) / 256.0;
    let v = 1.0 - exp(-hits * EXPOSURE);
    let col = mix(COLD_COLOR, HOT_COLOR, v) * v;
    return vec4<f32>(col, 1.0);
}
