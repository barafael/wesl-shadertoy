// Lygia Marble — paper marbling: paint drops on a swirling bath, pushed
// around without ever blurring. Drag to stir.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, fluid, mouse
// Built on LYGIA's `distort::marble` (https://lygia.xyz). Instead of moving
// the paint every frame, which would smear it a little more each time, it
// moves a map that says where each pixel's paint came from:
//   buffer_a  the ink. It stays put, except once every PERIOD frames
//             (`marbleBake`), when it is read once through the map and a
//             new drop of paint lands.
//   buffer_b  the map, packed into a color (`marbleEncode`). It restarts as
//             the identity after each bake (`marbleReset`) and is carried
//             back along the flow every frame.
// The image pass shows the ink read through the current map (`marble`).
import package::shadertoy::{i_time, i_resolution, i_mouse, i_frame};
import package::shadertoy::{i_channel_0, i_channel_1, i_sampler};
import lygia::distort::marble::{marble, marbleEncode, marbleDecode, marbleReset, marbleBake};
import lygia::generative::snoise::snoise3;
import lygia::generative::random::{random, random21};
import lygia::math::gaussian::gaussian2;
import lygia::color::palette::pigments::{CADMIUM_RED, ULTRAMARINE_BLUE, YELLOW_OCHRE, PHTHALO_GREEN, COBALT_VIOLET, CADMIUM_ORANGE, IVORY_BLACK};
import lygia::color::space::srgb2rgb::srgb2rgb;

// Frames between bakes, each of which also drops new paint.
const PERIOD: f32 = 45.0; // tweak: 10..240
// Speed of the swirl, in pixels per frame.
const FLOW: f32 = 0.6; // tweak: 0..5
const SWIRL_SCALE: f32 = 1.5; // tweak: 0.3..6
const STIR: f32 = 3.0; // tweak: 0..10
const STIR_RADIUS: f32 = 0.08; // tweak: 0.02..0.3
const DROP_SIZE: f32 = 0.06; // tweak: 0.01..0.2
const STRIPES: f32 = 9.0; // tweak: 2..30
// The bath, in sRGB like the pigments.
const BATH_COLOR: vec3<f32> = vec3<f32>(0.93, 0.89, 0.8);

fn pigment(i: f32) -> vec3<f32> {
    switch i32(i) % 7 {
        case 0: { return CADMIUM_RED; }
        case 1: { return ULTRAMARINE_BLUE; }
        case 2: { return YELLOW_OCHRE; }
        case 3: { return PHTHALO_GREEN; }
        case 4: { return COBALT_VIOLET; }
        case 5: { return CADMIUM_ORANGE; }
        default: { return IVORY_BLACK; }
    }
}

/// The map from the previous frame at `p` (pixels), filtered bilinearly by
/// hand: the buffers are float textures, which only sample nearest. The
/// decode is linear, so mixing decoded texels is exact.
fn map_at(p: vec2<f32>) -> vec2<f32> {
    let size = vec2<i32>(i_resolution().xy) - 1;
    let q = p - 0.5;
    let i = vec2<i32>(floor(q));
    let f = fract(q);
    let a = marbleDecode(textureLoad(i_channel_1, clamp(i, vec2<i32>(0), size), 0));
    let b = marbleDecode(textureLoad(i_channel_1, clamp(i + vec2<i32>(1, 0), vec2<i32>(0), size), 0));
    let c = marbleDecode(textureLoad(i_channel_1, clamp(i + vec2<i32>(0, 1), vec2<i32>(0), size), 0));
    let d = marbleDecode(textureLoad(i_channel_1, clamp(i + vec2<i32>(1, 1), vec2<i32>(0), size), 0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

/// The ink from the previous frame at `uv`, filtered bilinearly by hand.
/// `marble()` samples nearest, which is fine for showing the ink but would
/// add up to speckles if every bake resampled it that way.
fn ink_at(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(i_resolution().xy) - 1;
    let q = uv * i_resolution().xy - 0.5;
    let i = vec2<i32>(floor(q));
    let f = fract(q);
    let a = textureLoad(i_channel_0, clamp(i, vec2<i32>(0), size), 0).rgb;
    let b = textureLoad(i_channel_0, clamp(i + vec2<i32>(1, 0), vec2<i32>(0), size), 0).rgb;
    let c = textureLoad(i_channel_0, clamp(i + vec2<i32>(0, 1), vec2<i32>(0), size), 0).rgb;
    let d = textureLoad(i_channel_0, clamp(i + vec2<i32>(1, 1), vec2<i32>(0), size), 0).rgb;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

/// The bath's flow at `p` (pixels, y down), in pixels per frame: the curl
/// of a slowly changing noise, so it swirls without sources or sinks, plus a
/// vortex around the pointer while dragging.
fn flow(p: vec2<f32>) -> vec2<f32> {
    let res = i_resolution().xy;
    let q = p / res.y * SWIRL_SCALE;
    let t = 0.03 * i_time();
    let e = 0.01;
    let dx = snoise3(vec3<f32>(q + vec2<f32>(e, 0.0), t)) - snoise3(vec3<f32>(q - vec2<f32>(e, 0.0), t));
    let dy = snoise3(vec3<f32>(q + vec2<f32>(0.0, e), t)) - snoise3(vec3<f32>(q - vec2<f32>(0.0, e), t));
    var v = FLOW * vec2<f32>(dy, -dx) / (2.0 * e) * 0.3;
    let mouse = i_mouse();
    if mouse.z > 0.0 {
        let m = vec2<f32>(mouse.x, res.y - mouse.y);
        let r = (p - m) / res.y;
        v += STIR * gaussian2(r, STIR_RADIUS) * vec2<f32>(-r.y, r.x) / (length(r) + 1e-3);
    }
    return v;
}

// Ink, in sRGB.
fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = frag_coord / res;
    let frame = f32(i_frame());
    if frame < 1.0 {
        // Start from combed stripes of paint.
        let band = floor(uv.y * STRIPES);
        let paint = select(BATH_COLOR, pigment(band), band % 2.0 > 0.5);
        return vec4<f32>(paint, 1.0);
    }
    var ink = textureLoad(i_channel_0, vec2<i32>(frag_coord), 0);
    if marbleBake(frame, PERIOD) {
        // Bake: read the ink once through the map.
        ink = vec4<f32>(ink_at(map_at(frag_coord)), 1.0);
        // A fresh drop: a disc with a ring of bath color inside.
        let cycle = floor(frame / PERIOD);
        let center = 0.1 + 0.8 * random21(cycle + 0.5);
        let r = DROP_SIZE * (0.6 + 0.8 * random(cycle + 7.1));
        let d = length((uv - center) * vec2<f32>(res.x / res.y, 1.0));
        let px = 1.0 / res.y;
        let disc = 1.0 - smoothstep(r - px, r + px, d);
        let ring = 1.0 - smoothstep(0.1 * r, 0.1 * r + px, abs(d - 0.55 * r));
        ink = vec4<f32>(mix(ink.rgb, mix(pigment(cycle + 3.0), BATH_COLOR, ring), disc), 1.0);
    }
    return ink;
}

// The map: where each pixel's paint came from, in uv.
fn buffer_b(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    if marbleReset(f32(i_frame()), PERIOD) {
        return marbleEncode(frag_coord / res);
    }
    return marbleEncode(map_at(frag_coord - flow(frag_coord)));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    // The buffers are stored y down; frag_coord points up.
    let p = vec2<f32>(frag_coord.x, res.y - frag_coord.y);
    var ink: vec3<f32>;
    if marbleBake(f32(i_frame()), PERIOD) {
        // The ink was just displaced by the map: show it as is.
        ink = textureLoad(i_channel_0, vec2<i32>(p), 0).rgb;
    } else {
        ink = marble(i_channel_0, i_channel_1, i_sampler, p / res).rgb;
    }
    return vec4<f32>(srgb2rgb(ink), 1.0);
}
