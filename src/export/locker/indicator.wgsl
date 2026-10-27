// The password indicator drawn over the shader: a dark pill low on the
// screen with a dot per typed character, its rim white while typing, blue
// while checking and red after a wrong password; an orange bar under it
// while Caps Lock is on, and under that what PAM said, white on a dark
// backing (rasterized into `text` by the locker). `plain` is the background when the shader cannot
// be built on this GPU.

struct Indicator {
    // xy: surface size in pixels, z: pixels per logical pixel, w: opacity.
    frame: vec4<f32>,
    // x: typed characters, y: 0 typing, 1 checking, 2 wrong, z: caps lock,
    // w: seconds since locking.
    status: vec4<f32>,
    // xy: size of `text` in pixels, 0 when PAM said nothing.
    message: vec4<f32>,
}

@group(0) @binding(0) var<uniform> ui: Indicator;
@group(0) @binding(1) var text: texture_2d<f32>;

const MAX_DOTS: f32 = 24.0;

// Signed distance to a box of half size `half` with corners of radius `r`.
fn rounded(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// The same with fully rounded ends.
fn pill(p: vec2<f32>, half: vec2<f32>) -> f32 {
    return rounded(p, half, half.y);
}

// `rgb` at coverage `a` over the premultiplied `under`.
fn over(under: vec4<f32>, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a + under.rgb * (1.0 - a), a + under.a * (1.0 - a));
}

@fragment
fn indicator(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = ui.frame.xy;
    let s = ui.frame.z;
    let typed = ui.status.x;
    let state = ui.status.y;
    let t = ui.status.w;

    let dots = min(typed, MAX_DOTS);
    let spacing = 18.0 * s;
    let half = vec2<f32>(max(dots, 4.0) * spacing * 0.5 + 18.0 * s, 22.0 * s);
    let center = vec2<f32>(size.x * 0.5, size.y * 0.75);
    let p = position.xy - center;
    let aa = 1.0;

    var rim = vec3<f32>(0.9);
    if state == 1.0 {
        rim = mix(vec3<f32>(0.15, 0.35, 1.0), vec3<f32>(0.5, 0.7, 1.0), 0.5 + 0.5 * sin(t * 6.0));
    } else if state == 2.0 {
        rim = vec3<f32>(1.0, 0.12, 0.1);
    }

    let d = pill(p, half);
    var color = over(vec4<f32>(0.0), vec3<f32>(0.01), 0.6 * clamp(0.5 - d / aa, 0.0, 1.0));
    let ring = abs(d + 1.5 * s) - 1.5 * s;
    color = over(color, rim, clamp(0.5 - ring / aa, 0.0, 1.0));

    for (var i = 0.0; i < dots; i += 1.0) {
        let x = (i - (dots - 1.0) * 0.5) * spacing;
        let disc = length(p - vec2<f32>(x, 0.0)) - 5.0 * s;
        color = over(color, rim, clamp(0.5 - disc / aa, 0.0, 1.0));
    }

    if ui.status.z > 0.5 {
        let bar = pill(p - vec2<f32>(0.0, half.y + 12.0 * s), vec2<f32>(14.0 * s, 2.0 * s));
        color = over(color, vec3<f32>(1.0, 0.55, 0.05), clamp(0.5 - bar / aa, 0.0, 1.0));
    }

    let text_size = ui.message.xy;
    if text_size.x > 0.0 {
        let top = half.y + 26.0 * s;
        let box_center = vec2<f32>(0.0, top + text_size.y * 0.5);
        let pad = vec2<f32>(12.0, 8.0) * s;
        let backing = rounded(p - box_center, text_size * 0.5 + pad, 10.0 * s);
        color = over(color, vec3<f32>(0.01), 0.7 * clamp(0.5 - backing / aa, 0.0, 1.0));
        let texel = floor(p - vec2<f32>(-text_size.x * 0.5, top));
        if all(texel >= vec2<f32>(0.0)) && all(texel < text_size) {
            let coverage = textureLoad(text, vec2<i32>(texel), 0).r;
            color = over(color, vec3<f32>(0.95), coverage);
        }
    }

    return color * ui.frame.w;
}

@fragment
fn plain() -> @location(0) vec4<f32> {
    return vec4<f32>(0.02, 0.02, 0.03, 1.0);
}
