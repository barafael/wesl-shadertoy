// Raymarch — a bobbing sphere over a checkered ground, lit and mouse-steered.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 3d, raymarching, beginner
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;
import package::shadertoy::i_mouse;

const STEPS: i32 = 96; // tweak: 8..256
const BOB_SPEED: f32 = 2.0;
const SPHERE_RADIUS: f32 = 0.8; // tweak: 0.1..1.5
const SPHERE_COLOR: vec3<f32> = vec3<f32>(0.9, 0.4, 0.2);
const CHECKER_SCALE: f32 = 2.0; // checker tiles per unit
const LIGHT_DIR: vec3<f32> = vec3<f32>(0.6, 0.9, -0.4); // tweak: -1..1
const SKY_HORIZON: vec3<f32> = vec3<f32>(0.1, 0.12, 0.2); // tweak: color
const SKY_ZENITH: vec3<f32> = vec3<f32>(0.5, 0.6, 0.8); // tweak: color

fn sd_sphere(p: vec3<f32>, c: vec3<f32>, r: f32) -> f32 {
    return length(p - c) - r;
}

// Returns (distance, material id).
fn map(p: vec3<f32>) -> vec2<f32> {
    let t = i_time();
    let bob = vec3<f32>(0.0, 0.4 + 0.2 * sin(t * BOB_SPEED), 0.0);
    let d_sphere = sd_sphere(p, bob, SPHERE_RADIUS);
    let d_ground = p.y - (-0.5);
    if (d_sphere < d_ground) {
        return vec2<f32>(d_sphere, 1.0);
    }
    return vec2<f32>(d_ground, 2.0);
}

fn normal(p: vec3<f32>) -> vec3<f32> {
    let e = 0.001;
    let d = map(p).x;
    return normalize(vec3<f32>(
        map(p + vec3<f32>(e, 0.0, 0.0)).x - d,
        map(p + vec3<f32>(0.0, e, 0.0)).x - d,
        map(p + vec3<f32>(0.0, 0.0, e)).x - d,
    ));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let yaw = 0.4 + i_mouse().x * 0.01;
    let ro = vec3<f32>(2.5 * sin(yaw), 1.2, 2.5 * cos(yaw));
    let rd = normalize(vec3<f32>(uv.x, uv.y - 0.15, -1.5));

    var t = 0.0;
    var hit_id = 0.0;
    var p = ro;
    for (var i = 0; i < STEPS; i = i + 1) {
        p = ro + rd * t;
        let h = map(p);
        if (h.x < 0.001) {
            hit_id = h.y;
            break;
        }
        t = t + h.x;
        if (t > 20.0) {
            break;
        }
    }

    if (hit_id == 0.0) {
        let sky = mix(SKY_HORIZON, SKY_ZENITH, clamp(rd.y + 0.4, 0.0, 1.0));
        return vec4<f32>(sky, 1.0);
    }

    let n = normal(p);
    let light_dir = normalize(LIGHT_DIR);
    let diff = max(dot(n, light_dir), 0.0);
    var albedo = SPHERE_COLOR;
    if (hit_id > 1.5) {
        let chk = step(0.5, fract(p.x * CHECKER_SCALE)) + step(0.5, fract(p.z * CHECKER_SCALE));
        albedo = mix(vec3<f32>(0.15), vec3<f32>(0.8), fract(chk * 0.5));
    }
    let col = albedo * (0.25 + 0.75 * diff);
    return vec4<f32>(col, 1.0);
}
