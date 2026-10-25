// Elevated — fly over fractal terrain, with snow on the flats.
// author: wesl-shadertoy, after Elevated by Inigo Quilez
// license: MIT OR Apache-2.0
// tags: 3d, raymarching, noise
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

const FLY_SPEED: f32 = 1.5;
const TERRAIN_SCALE: f32 = 0.25; // horizontal frequency of the hills
const TERRAIN_HEIGHT: f32 = 3.0;
const STEPS: i32 = 90; // tweak: 16..256
const SUN_DIR: vec3<f32> = vec3<f32>(0.4, 0.8, 0.2); // tweak: -1..1
const GRASS_COLOR: vec3<f32> = vec3<f32>(0.2, 0.35, 0.15);
const ROCK_COLOR: vec3<f32> = vec3<f32>(0.5, 0.45, 0.35);
const SNOW_COLOR: vec3<f32> = vec3<f32>(0.95, 0.95, 0.95);
const HAZE: f32 = 0.0006; // tweak: 0..0.005

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
    for (var i = 0; i < 5; i = i + 1) {
        v = v + a * noise(p);
        p = p * 2.0 + vec2<f32>(17.0, 3.0);
        a = a * 0.5;
    }
    return v;
}

// Heightfield: not a true SDF, so the march below takes half-steps.
fn terrain(p: vec3<f32>) -> f32 {
    let h = fbm(p.xz * TERRAIN_SCALE) * TERRAIN_HEIGHT - 1.2;
    return p.y - h;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    let ro = vec3<f32>(0.0, 1.6 + 0.3 * sin(t * 0.4), t * FLY_SPEED);
    let rd = normalize(vec3<f32>(uv.x, uv.y - 0.25, 1.0));

    var tm = 0.0;
    var p = ro;
    var hit = 0.0;
    for (var i = 0; i < STEPS; i = i + 1) {
        p = ro + rd * tm;
        let d = terrain(p);
        if (d < 0.01) {
            hit = 1.0;
            break;
        }
        tm = tm + max(d * 0.5, 0.02);
        if (tm > 60.0) {
            break;
        }
    }

    let sky = mix(
        vec3<f32>(0.55, 0.7, 0.9),
        vec3<f32>(0.15, 0.25, 0.5),
        clamp(1.0 - rd.y * 2.0, 0.0, 1.0),
    );
    if (hit < 0.5) {
        return vec4<f32>(sky, 1.0);
    }

    let e = 0.05;
    let n = normalize(vec3<f32>(
        terrain(p) - terrain(p + vec3<f32>(e, 0.0, 0.0)),
        terrain(p) - terrain(p + vec3<f32>(0.0, e, 0.0)),
        terrain(p) - terrain(p + vec3<f32>(0.0, 0.0, e)),
    ));
    let diff = max(dot(n, normalize(SUN_DIR)), 0.0);

    var col = mix(GRASS_COLOR, ROCK_COLOR, clamp(p.y * 0.4 + 0.3, 0.0, 1.0));
    // Snow settles on the flats of the high ground.
    col = mix(col, SNOW_COLOR, smoothstep(0.6, 1.2, p.y) * step(0.7, n.y));
    col = col * (0.35 + 0.65 * diff);
    col = mix(col, sky, 1.0 - exp(-tm * tm * HAZE));
    return vec4<f32>(col, 1.0);
}
