// Liquid Toy — https://www.shadertoy.com/view/fljBWc by leon (CC BY-NC 2.0)
// Ported from eliotbo/bevy_shadertoy_wgsl: buffer_a holds the paint field,
// advected each frame along the normal of its own height (a feedback
// "liquid metal" effect) and stirred by 3D simplex noise; the image pass
// shades that field as a specular liquid with a rainbow tint. The original
// dither texture is replaced by a per-pixel hash. Drag the mouse to move
// the emitter; click near the left edge for the buffer/normal/tint debug view.

import package::shadertoy::i_channel_0;
import package::shadertoy::i_sampler;
import package::shadertoy::i_mouse;
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;
import package::shadertoy::i_time_delta;

const speed: f32 = 0.01;
const scale: f32 = 0.1;
const falloff: f32 = 3.;
const fade: f32 = 0.4;
const strength: f32 = 1.;
const range: f32 = 5.;

fn random3(c: vec3<f32>) -> vec3<f32> {
    var j = 4096. * sin(dot(c, vec3<f32>(17., 59.4, 15.)));
    var r = vec3<f32>(0.);
    r.z = fract(512. * j);
    j = j * 0.125;
    r.x = fract(512. * j);
    j = j * 0.125;
    r.y = fract(512. * j);
    return r - 0.5;
}

const F3: f32 = 0.3333333;
const G3: f32 = 0.1666667;

fn simplex3d(p: vec3<f32>) -> f32 {
    let s = floor(p + dot(p, vec3<f32>(F3)));
    let x = p - s + dot(s, vec3<f32>(G3));
    let e = step(vec3<f32>(0.), x - x.yzx);
    let i1 = e * (1. - e.zxy);
    let i2 = 1. - e.zxy * (1. - e);
    let x1 = x - i1 + G3;
    let x2 = x - i2 + 2. * G3;
    let x3 = x - 1. + 3. * G3;

    var w = vec4<f32>(dot(x, x), dot(x1, x1), dot(x2, x2), dot(x3, x3));
    w = max(0.6 - w, vec4<f32>(0.));
    let d = vec4<f32>(
        dot(random3(s), x),
        dot(random3(s + i1), x1),
        dot(random3(s + i2), x2),
        dot(random3(s + 1.), x3),
    );
    w = w * w;
    w = w * w;
    return dot(d * w, vec4<f32>(52.));
}

fn dummy(p3: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(0.5 + 0.5 * simplex3d(p3 * 16.));
}

fn fbm(p: vec3<f32>) -> vec3<f32> {
    var result = vec3<f32>(0.);
    var amplitude = 0.5;
    for (var index: f32 = 0.; index < 3.; index = index + 1.) {
        result = result + dummy(p / amplitude) * amplitude;
        amplitude = amplitude / falloff;
    }
    return result;
}

// The paint field, sampled with hardware bilinear filtering (the reference
// engine hand-rolled it with textureLoad).
fn field(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(i_channel_0, i_sampler, uv, 0.);
}

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    var uv = (frag_coord - R / 2.) / R.y;
    var spice = fbm(vec3<f32>(uv * scale, i_time() * speed));

    // The emitter follows the mouse, or wanders in a circle by itself.
    let m = i_mouse();
    let mouse = (m.xy - R / 2.) / R.y;
    let t = i_time() * 2.;
    if (m.z > 0.5) {
        uv = uv - mouse;
    } else {
        uv = uv - vec2<f32>(cos(t), sin(t)) * 0.3;
    }
    var paint = smoothstep(0.1, 0., length(uv));

    uv = frag_coord / R;
    let data = field(uv);
    let unit = vec3<f32>(range / R, 0.);
    let normal = normalize(
        vec3<f32>(
            field(uv - unit.xz).r - field(uv + unit.xz).r,
            field(uv - unit.zy).r - field(uv + unit.zy).r,
            data.x * data.x,
        ) + 0.001,
    );

    // Advect along the surface normal, plus a noise-driven swirl.
    var offset = -normal.xy;
    spice.x = spice.x * (6.28 * 2.) + i_time();
    offset = offset + vec2<f32>(cos(spice.x), sin(spice.x));
    let frame = field(uv + strength * offset / R);

    paint = max(paint, frame.x - i_time_delta() * fade);
    return vec4<f32>(clamp(paint, 0., 1.));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let R = i_resolution().xy;
    let uv = frag_coord / R;

    // Stand-in for the original's blue-noise dither texture.
    let dither = fract(sin(dot(frag_coord, vec2<f32>(12.9898, 78.233))) * 43758.5453);

    let gray = field(uv).x;
    let glow_range = 3.;
    let unit = vec3<f32>(glow_range / R, 0.);
    let normal = normalize(
        vec3<f32>(
            field(uv + unit.xz).r - field(uv - unit.xz).r,
            field(uv - unit.zy).r - field(uv + unit.zy).r,
            gray * gray * gray,
        ),
    );

    var color = vec3<f32>(0.3) * (1. - abs(dot(normal, vec3<f32>(0., 0., 1.))));
    let dir = normalize(vec3<f32>(0., 1., 2.));
    let specular = pow(dot(normal, dir) * 0.5 + 0.5, 20.);
    color = color + vec3<f32>(0.5) * smoothstep(0.2, 1., specular);
    let tint =
        0.5 + 0.5 * cos(vec3<f32>(1., 2., 3.) + dot(normal, dir) * 4. - uv.y * 3. - 3.);
    color = color + tint * smoothstep(0.15, 0., gray);
    color = color - dither * 0.1;

    let background = vec3<f32>(1.) * smoothstep(1.5, -0.5, length(uv - 0.5));
    color = mix(background, clamp(color, vec3<f32>(0.), vec3<f32>(1.)), smoothstep(0.01, 0.1, gray));

    // Debug view: click near the left edge to inspect the passes.
    let m = i_mouse();
    if (m.z > 0.5 && m.x / R.x < 0.1) {
        if (uv.x < 0.33) {
            color = vec3<f32>(gray);
        } else if (uv.x < 0.66) {
            color = normal * 0.5 + 0.5;
        } else {
            color = tint;
        }
    }
    return vec4<f32>(color, 1.);
}
