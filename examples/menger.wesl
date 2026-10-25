// Menger — the classic sponge fractal, raymarched with a slow spin.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 3d, fractal, raymarching
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

const LEVELS: i32 = 4; // tweak: 0..6
const STEPS: i32 = 110; // tweak: 16..256
const SPIN: f32 = 0.3; // spin speed, radians per second
const TILT: f32 = 0.65; // tweak: -1.6..1.6
const CAMERA_DIST: f32 = 3.4; // tweak: 1.5..8
const LIGHT_DIR: vec3<f32> = vec3<f32>(0.5, 0.8, -0.6); // tweak: -1..1
const SURFACE_COLOR: vec3<f32> = vec3<f32>(0.75, 0.78, 0.85);
const BG_COLOR: vec3<f32> = vec3<f32>(0.02, 0.02, 0.04);

fn sd_box(p: vec3<f32>, b: f32) -> f32 {
    let q = abs(p) - vec3<f32>(b);
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

// Box minus three orthogonal square tunnels, folded at LEVELS scales.
fn sd_menger(p_in: vec3<f32>) -> f32 {
    var p = p_in;
    var s = 1.0;
    var d = sd_box(p, 1.0);
    for (var i = 0; i < LEVELS; i = i + 1) {
        // `mod` is reserved by WESL, so fold into [-1, 1) by hand.
        let w = p * s + vec3<f32>(1.0);
        let q = w - 2.0 * floor(w * 0.5) - vec3<f32>(1.0);
        s = s * 3.0;
        let hx = max(abs(q.x), abs(q.y)) - 1.0 / 3.0;
        let hy = max(abs(q.y), abs(q.z)) - 1.0 / 3.0;
        let hz = max(abs(q.z), abs(q.x)) - 1.0 / 3.0;
        let hole = min(hx, min(hy, hz)) / s;
        d = max(d, -hole);
    }
    return d;
}

fn rot_y(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * p.x + s * p.z, p.y, -s * p.x + c * p.z);
}

fn rot_x(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(p.x, c * p.y - s * p.z, s * p.y + c * p.z);
}

fn map(p: vec3<f32>) -> f32 {
    return sd_menger(rot_x(rot_y(p, i_time() * SPIN), TILT));
}

fn normal(p: vec3<f32>) -> vec3<f32> {
    let e = vec3<f32>(0.001, 0.0, 0.0);
    return normalize(vec3<f32>(
        map(p + e.xyy) - map(p - e.xyy),
        map(p + e.yxy) - map(p - e.yxy),
        map(p + e.yyx) - map(p - e.yyx),
    ));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let ro = vec3<f32>(0.0, 0.0, -CAMERA_DIST);
    let rd = normalize(vec3<f32>(uv, 1.6));

    var t = 0.0;
    var p = ro;
    var hit = 0.0;
    for (var i = 0; i < STEPS; i = i + 1) {
        p = ro + rd * t;
        let d = map(p);
        if (d < 0.0008) {
            hit = 1.0;
            break;
        }
        t = t + d;
        if (t > 8.0) {
            break;
        }
    }

    if (hit < 0.5) {
        return vec4<f32>(BG_COLOR, 1.0);
    }

    let n = normal(p);
    let diff = max(dot(n, normalize(LIGHT_DIR)), 0.0);
    let fog = 1.0 - clamp(t / 8.0, 0.0, 1.0);
    let col = SURFACE_COLOR * (0.15 + 0.85 * diff) * fog;
    return vec4<f32>(col, 1.0);
}
