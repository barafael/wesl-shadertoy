// Fire — the classic campfire: fbm scrolling down a blackbody palette.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const RISE_SPEED: f32 = 0.9;
const NOISE_SCALE: f32 = 3.5;
const OCTAVES: i32 = 5; // tweak: 1..8
// Screen height (from the center) where the flames have died out.
const FLAME_TOP: f32 = 0.5; // tweak: -0.5..1
const GAIN: f32 = 2.4; // tweak: 0..5
const CUTOFF: f32 = 0.55; // tweak: 0..1.5
// The blackbody ramp, coolest to hottest.
const EMBER_COLOR: vec3<f32> = vec3<f32>(0.05, 0.0, 0.0);
const FLAME_COLOR: vec3<f32> = vec3<f32>(0.9, 0.15, 0.0);
const GLOW_COLOR: vec3<f32> = vec3<f32>(1.0, 0.6, 0.05);
const CORE_COLOR: vec3<f32> = vec3<f32>(1.0, 1.0, 0.6);

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y,
    );
}

fn fbm(p_in: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var p = p_in;
    for (var i = 0; i < OCTAVES; i = i + 1) {
        v = v + a * noise(p);
        p = p * 2.0 + vec2<f32>(13.0, 7.0);
        a = a * 0.5;
    }
    return v;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    // Scroll the noise toward +y (screen up) so the flames rise.
    let q = vec2<f32>(uv.x, uv.y - t * RISE_SPEED);
    var f = fbm(q * NOISE_SCALE);
    // Hot at the bottom of the screen, dying out toward the top.
    f = f * clamp((FLAME_TOP - uv.y) * 2.0, 0.0, 1.0);
    f = clamp(f * GAIN - CUTOFF, 0.0, 1.0);

    var col = mix(EMBER_COLOR, FLAME_COLOR, smoothstep(0.0, 0.5, f));
    col = mix(col, GLOW_COLOR, smoothstep(0.45, 0.75, f));
    col = mix(col, CORE_COLOR, smoothstep(0.75, 1.0, f));
    return vec4<f32>(col, 1.0);
}
