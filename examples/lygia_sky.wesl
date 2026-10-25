// Lygia Sky — one procedural sky through two projections: an equirectangular
// panorama and a fisheye looking straight up.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 3d, atmosphere, mouse
// The sky is a function of view direction only: a sun that rises and sets,
// a horizon glow and simplex-noise clouds on a flat layer overhead.
// LYGIA's `space::equirect2xyz` and `space::fisheye2xyz`
// (https://lygia.xyz) turn the two panels' pixels into directions. Drag to
// turn and to scrub the time of day.
import package::shadertoy::{i_time, i_resolution, i_mouse};
import lygia::space::equirect2xyz::equirect2xyz;
import lygia::space::fisheye2xyz::fisheye2xyz;
import lygia::generative::snoise::snoise3;
import lygia::color::tonemap::aces::tonemapACES3;

// Seconds for a full day.
const DAY: f32 = 30.0; // tweak: 5..120
const CLOUD_COVER: f32 = 0.35; // tweak: 0..1
const CLOUD_SCALE: f32 = 1.2; // tweak: 0.2..4
const WIND: f32 = 0.05; // tweak: 0..0.5
const EXPOSURE: f32 = 1.0; // tweak: 0.1..4
// How far the sun sinks at night: 1 gives half a day of darkness.
const NIGHT_DIP: f32 = 0.6; // tweak: 0..1
const ZENITH_COLOR: vec3<f32> = vec3<f32>(0.08, 0.2, 0.55);
const HORIZON_COLOR: vec3<f32> = vec3<f32>(0.55, 0.65, 0.8);
const SUNSET_COLOR: vec3<f32> = vec3<f32>(1.2, 0.45, 0.15);
const NIGHT_COLOR: vec3<f32> = vec3<f32>(0.02, 0.035, 0.08);
const GROUND_COLOR: vec3<f32> = vec3<f32>(0.06, 0.05, 0.04);
const SUN_COLOR: vec3<f32> = vec3<f32>(20.0, 17.0, 13.0);

fn fbm(p: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        v += a * snoise3(q);
        q = q * 2.03 + 1.7;
        a *= 0.5;
    }
    return v;
}

fn sky(dir: vec3<f32>, sun: vec3<f32>, t: f32) -> vec3<f32> {
    let day = smoothstep(-0.15, 0.2, sun.y);
    let low = 1.0 - smoothstep(0.0, 0.5, sun.y);
    let up = max(dir.y, 0.0);
    var col = mix(HORIZON_COLOR, ZENITH_COLOR, pow(up, 0.5));
    // The horizon glows toward the sun as it sets.
    let toward = max(dot(normalize(dir.xz), normalize(sun.xz + 1e-5)), 0.0);
    col = mix(col, SUNSET_COLOR, low * pow(toward, 3.0) * (1.0 - up) * day);
    col = mix(NIGHT_COLOR, col, day);
    col += SUN_COLOR * smoothstep(0.9995, 0.9998, dot(dir, sun)) * day;
    col += 0.3 * SUNSET_COLOR * pow(max(dot(dir, sun), 0.0), 64.0) * day;

    // Clouds on a plane above: project the ray onto it.
    if dir.y > 0.02 {
        let p = dir.xz / dir.y * CLOUD_SCALE;
        let n = fbm(vec3<f32>(p + WIND * t, 0.02 * t));
        let cover = smoothstep(1.0 - 2.0 * CLOUD_COVER, 1.0, 0.5 + n);
        let lit = mix(vec3<f32>(0.9), SUNSET_COLOR, 0.7 * low * day) * mix(0.05, 1.0, day);
        // Fade them out toward the horizon, where they would alias.
        col = mix(col, lit, cover * smoothstep(0.03, 0.3, dir.y));
    }
    if dir.y < 0.0 {
        col = mix(col, GROUND_COLOR * (0.25 + day), smoothstep(0.0, -0.02, dir.y));
    }
    return col;
}

fn yawed(d: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let t = i_time();
    var yaw = 0.03 * t;
    // Start mid-morning.
    var hour = 0.35 + t / DAY;
    let mouse = i_mouse();
    if mouse.z > 0.0 {
        yaw = 6.2831853 * mouse.x / res.x;
        hour = mouse.y / res.y;
    }
    // The sun circles on a raised, tilted path: high at noon, only briefly
    // below the horizon around midnight.
    let a = 6.2831853 * (hour - 0.25);
    let sun = normalize(vec3<f32>(cos(a), NIGHT_DIP * sin(a) + (1.0 - NIGHT_DIP), 0.3 + 0.4 * sin(a)));

    // A 2:1 panorama on the left, a round fisheye on the right, as large
    // as both fit side by side.
    let margin = 0.04 * res.y;
    let h = min((res.x - 3.0 * margin) / 3.0, res.y - 2.0 * margin);
    let pano = vec2<f32>(2.0 * h, h);
    let dome = h;
    var col = vec3<f32>(0.016, 0.015, 0.014);
    let pp = (frag_coord - vec2<f32>(margin, 0.5 * (res.y - h))) / pano;
    if all(pp >= vec2<f32>(0.0)) && all(pp <= vec2<f32>(1.0)) {
        col = tonemapACES3(EXPOSURE * sky(yawed(equirect2xyz(pp), yaw), sun, t));
    }
    let center = vec2<f32>(2.0 * margin + 2.5 * h, 0.5 * res.y);
    let fp = (frag_coord - center) / dome + 0.5;
    if length(fp - 0.5) < 0.5 {
        col = tonemapACES3(EXPOSURE * sky(yawed(fisheye2xyz(fp), yaw), sun, t));
    }
    return vec4<f32>(col, 1.0);
}
