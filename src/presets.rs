//! Built-in example shaders, written in the wesl-shadertoy dialect: the
//! editor text is the `package::user` module, so each preset starts with the
//! `import package::shadertoy::...` lines that pull in the uniforms it uses.
//!
//! The hand-written examples live below as raw strings; the shadertoy ports
//! live as standalone files in `presets/` (each carries its own attribution
//! header) and are pulled in with `include_str!`.

pub struct Preset {
    pub name: &'static str,
    pub code: &'static str,
}

const PLASMA: &str = r#"// Plasma — the classic cosine palette, driven by i_time.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

const SPEED: f32 = 1.0;
const ZOOM: f32 = 1.0; // pattern scale
// Per-channel phase of the cosine palette.
const PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3
const BRIGHTNESS: f32 = 0.5; // tweak: 0..1
const CONTRAST: f32 = 0.5; // tweak: 0..1

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let col = BRIGHTNESS + CONTRAST * cos(t + uv.xyx * ZOOM + PHASE);
    return vec4<f32>(col, 1.0);
}
"#;

const RAYMARCH: &str = r#"// Raymarch — a bobbing sphere over a checkered ground, lit and mouse-steered.
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
"#;

const MANDELBROT: &str = r#"// Mandelbrot — breathing zoom into the seahorse valley.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const MAX_ITER: i32 = 128; // tweak: 16..512
const ZOOM_SPEED: f32 = 0.3;
// How far the zoom breathes, in e-folds either side of the middle.
const ZOOM_DEPTH: f32 = 5.0; // tweak: 0..8
const CENTER: vec2<f32> = vec2<f32>(-0.745, 0.113); // tweak: -2..1
const PALETTE_FREQ: f32 = 12.0; // color bands across the escape range
const PALETTE_PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let zoom = exp(sin(t * ZOOM_SPEED) * ZOOM_DEPTH + 3.0);
    let c = CENTER + (uv * 1.5) / zoom;

    var z = vec2<f32>(0.0, 0.0);
    var iters = 0.0;
    for (var k = 0; k < MAX_ITER; k = k + 1) {
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        if (dot(z, z) > 4.0) {
            break;
        }
        iters = iters + 1.0;
    }

    if (iters > f32(MAX_ITER) - 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let f = iters / f32(MAX_ITER);
    let col = 0.5 + 0.5 * cos(3.0 + f * PALETTE_FREQ + PALETTE_PHASE);
    return vec4<f32>(col, 1.0);
}
"#;

const JULIA: &str = r#"// Julia — the Mandelbrot's sibling, with c orbiting the main cardioid.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const MAX_ITER: i32 = 128; // tweak: 16..512
const ORBIT_RADIUS: f32 = 0.7885; // tweak: 0..1.2
const ORBIT_SPEED: f32 = 0.4;
const VIEW_SCALE: f32 = 1.6; // half-height of the view
const PALETTE_FREQ: f32 = 14.0; // color bands across the escape range
const PALETTE_PHASE: vec3<f32> = vec3<f32>(0.0, 2.0, 4.0); // tweak: 0..6.3

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let c = ORBIT_RADIUS * vec2<f32>(cos(t * ORBIT_SPEED), sin(t * ORBIT_SPEED));

    var z = uv * VIEW_SCALE;
    var iters = 0.0;
    for (var k = 0; k < MAX_ITER; k = k + 1) {
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        if (dot(z, z) > 4.0) {
            break;
        }
        iters = iters + 1.0;
    }

    if (iters > f32(MAX_ITER) - 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let f = iters / f32(MAX_ITER);
    let col = 0.5 + 0.5 * cos(2.0 + f * PALETTE_FREQ + PALETTE_PHASE);
    return vec4<f32>(col, 1.0);
}
"#;

const STAR_NEST: &str = r#"// Star Nest — Dave Hoskins' iconic fractal zoom, folded and re-lit each pass.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const SPEED: f32 = 0.15;
const ITERATIONS: i32 = 8; // tweak: 1..20
// The fold offset c circles this point...
const FOLD_CENTER: vec2<f32> = vec2<f32>(0.7, 0.85); // tweak: 0..1.5
// ...at this radius.
const WOBBLE: f32 = 0.1; // tweak: 0..0.5
const GLOW: f32 = 0.006; // tweak: 0..0.03
// Keeps the 1/d^2 spikes finite; smaller means sharper stars.
const GLOW_SOFTNESS: f32 = 0.0008; // tweak: 0.0001..0.005

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    var p = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let c = vec2<f32>(FOLD_CENTER.x + WOBBLE * cos(t), FOLD_CENTER.y + WOBBLE * sin(t));

    // The fold keeps p in a bounded region; each pass adds a 1/d^2 glow
    // spike, which is what makes the fractal stars blow out to white.
    var col = vec3<f32>(0.0);
    for (var k = 0; k < ITERATIONS; k = k + 1) {
        p = abs(p) / clamp(dot(p, p), 0.05, 1.0) - c;
        let d = length(p);
        let glow = GLOW / (d * d + GLOW_SOFTNESS);
        col = col + glow * vec3<f32>(
            0.5 + 0.5 * sin(3.0 * d + t),
            0.5 + 0.5 * sin(4.0 * d + t + 2.0),
            0.5 + 0.5 * sin(5.0 * d + t + 4.0),
        );
    }
    return vec4<f32>(col, 1.0);
}
"#;

const FIRE: &str = r#"// Fire — the classic campfire: fbm scrolling down a blackbody palette.
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
"#;

const VORONOI: &str = r#"// Voronoi — drifting cellular cells with dark seams between them.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const SPEED: f32 = 0.6;
const CELL_SCALE: f32 = 4.0; // cells per screen height
// The seams darken between these distances from the cell's site.
const SEAM_INNER: f32 = 0.15; // tweak: 0..1
const SEAM_OUTER: f32 = 0.45; // tweak: 0..1
const SEAM_DARKNESS: f32 = 0.8; // tweak: 0..1
const HUE_FREQ: f32 = 12.0; // how fast the color changes from cell to cell
const PALETTE_PHASE: vec3<f32> = vec3<f32>(0.5, 2.0, 3.5); // tweak: 0..6.3

fn hash2(p: vec2<f32>) -> vec2<f32> {
    return fract(
        sin(vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3))))
            * 43758.5453,
    );
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;

    let p = uv * CELL_SCALE;
    let ip = floor(p);
    let fp = fract(p);

    var d = 8.0;
    var cell = vec2<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let o = vec2<f32>(f32(i), f32(j));
            let h = hash2(ip + o);
            let pt = o + 0.5 + 0.5 * sin(t + 6.28318 * h) - fp;
            let dd = dot(pt, pt);
            if (dd < d) {
                d = dd;
                cell = h;
            }
        }
    }
    d = sqrt(d);

    let shade = 0.55 + 0.45 * sin(
        vec3<f32>(cell.x * HUE_FREQ, cell.y * HUE_FREQ, (cell.x + cell.y) * 9.0)
            + PALETTE_PHASE,
    );
    let col = shade * (1.0 - SEAM_DARKNESS * smoothstep(SEAM_INNER, SEAM_OUTER, d));
    return vec4<f32>(col, 1.0);
}
"#;

const METABALLS: &str = r#"// Metaballs — implicit 1/r² blobs that fuse, with a fresnel-ish glow.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const BALLS: i32 = 7; // tweak: 1..16
const SPEED: f32 = 1.0;
const ORBIT: f32 = 0.45; // tweak: 0..1
const STRENGTH: f32 = 0.02; // tweak: 0..0.1
const BG_COLOR: vec3<f32> = vec3<f32>(0.02, 0.03, 0.08);
const GLOW_COLOR: vec3<f32> = vec3<f32>(0.1, 0.5, 0.9);
const CORE_COLOR: vec3<f32> = vec3<f32>(0.9, 0.98, 1.0);

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;

    var f = 0.0;
    for (var i = 0; i < BALLS; i = i + 1) {
        let a = t * (0.5 + 0.1 * f32(i)) + f32(i) * 2.3;
        let c = vec2<f32>(sin(a), cos(a * 1.31)) * ORBIT;
        let d = max(dot(uv - c, uv - c), 0.0001);
        f = f + STRENGTH / d;
    }

    let glow = smoothstep(0.35, 0.95, f);
    let inside = smoothstep(0.9, 1.05, f);
    let col = BG_COLOR
        + GLOW_COLOR * glow * 0.6
        + CORE_COLOR * inside;
    return vec4<f32>(col, 1.0);
}
"#;

const TUNNEL: &str = r#"// Tunnel — the classic infinite corridor from the 1/r polar trick.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

const SPEED: f32 = 4.0;
const SEGMENTS: f32 = 4.0; // checker tiles per radian around the wall
const FOG: f32 = 0.12; // tweak: 0..0.5
const PALETTE_FREQ: f32 = 0.35; // color cycles along the tunnel
const LAMP_COLOR: vec3<f32> = vec3<f32>(0.6, 0.7, 1.0);
const LAMP_FALLOFF: f32 = 12.0; // larger means a smaller lamp
const LAMP_PULSE: f32 = 3.0; // pulse rate

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    let r = length(uv);
    let a = atan2(uv.y, uv.x);

    let depth = 1.0 / max(r, 0.0005);
    let z = depth + t * SPEED;
    let u = a * SEGMENTS;

    // WESL reserves `mod` for module declarations, so the modulo is spelled out.
    let s = floor(z * 0.5) + floor(u);
    let chk = s - 2.0 * floor(s * 0.5);
    let pal = 0.5 + 0.5 * cos(z * PALETTE_FREQ + vec3<f32>(0.0, 2.0, 4.0));
    var col = pal * (0.25 + 0.75 * chk);
    col = col * exp(-depth * FOG);
    // A lamp pulsing at the far end of the tunnel.
    col = col + LAMP_COLOR * exp(-r * LAMP_FALLOFF) * (0.5 + 0.5 * sin(t * LAMP_PULSE));
    return vec4<f32>(col, 1.0);
}
"#;

const MENGER: &str = r#"// Menger — the classic sponge fractal, raymarched with a slow spin.
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
"#;

const ELEVATED: &str = r#"// Elevated — TDM's classic: fly over fractal terrain, snow on the flats.
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
"#;

/// Gustavson's 2D gradient noise (MIT, Stefan Gustavson / munrocket gist,
/// as shipped in the wesl-rs `random_wgsl` example), inlined into a
/// domain-warped fBm landscape.
const PERLIN: &str = r#"// Perlin — Gustavson's 2D gradient noise (MIT), rolled into fBm and
// domain-warped: the noise field steers its own sample coordinates.
import package::shadertoy::{i_time, i_resolution};

const SPEED: f32 = 0.15;
const OCTAVES: i32 = 5; // tweak: 1..8
const SCALE: f32 = 3.0; // feature size of the landscape
const WARP: f32 = 1.5; // tweak: 0..4
const SEA_COLOR: vec3<f32> = vec3<f32>(0.05, 0.12, 0.24);
const LAND_COLOR: vec3<f32> = vec3<f32>(0.45, 0.35, 0.22);
const SNOW_COLOR: vec3<f32> = vec3<f32>(0.92, 0.90, 0.85);

fn permute4(x: vec4<f32>) -> vec4<f32> {
    return ((x * 34.0 + 1.0) * x) % vec4<f32>(289.0);
}

fn fade2(t: vec2<f32>) -> vec2<f32> {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn perlin_noise2(p: vec2<f32>) -> f32 {
    var pi: vec4<f32> = floor(p.xyxy) + vec4<f32>(0.0, 0.0, 1.0, 1.0);
    let pf = fract(p.xyxy) - vec4<f32>(0.0, 0.0, 1.0, 1.0);
    pi = pi % vec4<f32>(289.0); // keep the permutation free of truncation effects
    let i_x = pi.xzxz;
    let i_y = pi.yyww;
    let fx = pf.xzxz;
    let fy = pf.yyww;
    let i = permute4(permute4(i_x) + i_y);
    var gx: vec4<f32> = 2.0 * fract(i * 0.0243902439) - 1.0; // 1/41
    let gy = abs(gx) - 0.5;
    let tx = floor(gx + 0.5);
    gx = gx - tx;
    let g00 = vec2<f32>(gx.x, gy.x);
    let g10 = vec2<f32>(gx.y, gy.y);
    let g01 = vec2<f32>(gx.z, gy.z);
    let g11 = vec2<f32>(gx.w, gy.w);
    let norm = 1.79284291400159 - 0.85373472095314 * vec4<f32>(
        dot(g00, g00), dot(g01, g01), dot(g10, g10), dot(g11, g11),
    );
    let n00 = dot(g00 * norm.x, vec2<f32>(fx.x, fy.x));
    let n10 = dot(g10 * norm.y, vec2<f32>(fx.y, fy.y));
    let n01 = dot(g01 * norm.z, vec2<f32>(fx.z, fy.z));
    let n11 = dot(g11 * norm.w, vec2<f32>(fx.w, fy.w));
    let fade_xy = fade2(pf.xy);
    let n_x = mix(vec2<f32>(n00, n01), vec2<f32>(n10, n11), vec2<f32>(fade_xy.x));
    let n_xy = mix(n_x.x, n_x.y, fade_xy.y);
    return 2.3 * n_xy;
}

fn fbm(p: vec2<f32>) -> f32 {
    var value = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < OCTAVES; i = i + 1) {
        value = value + amp * perlin_noise2(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        amp = amp * 0.5;
    }
    return value;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * SPEED;
    let warp = vec2<f32>(
        fbm(uv * 2.0 + vec2<f32>(t, -t * 0.7)),
        fbm(uv * 2.0 + vec2<f32>(-t * 0.4, t)),
    );
    let f = fbm(uv * SCALE + WARP * warp + vec2<f32>(0.0, t * 0.5));
    var col = mix(SEA_COLOR, LAND_COLOR, smoothstep(-0.15, 0.25, f));
    col = mix(col, SNOW_COLOR, smoothstep(0.30, 0.55, f));
    col = col * (0.75 + 0.25 * smoothstep(-0.4, 0.4, warp.x + warp.y));
    return vec4<f32>(col, 1.0);
}
"#;

// Ports of classic shadertoy shaders, adapted to this dialect. The sources
// live in `presets/*.wgsl` so they can be edited and diffed as plain WGSL.
const SUNSET: &str = include_str!("../presets/sunset.wgsl");
const FIRE2: &str = include_str!("../presets/fire2.wgsl");
const SEASCAPE: &str = include_str!("../presets/seascape.wgsl");
const PROTEAN_CLOUDS: &str = include_str!("../presets/protean_clouds.wgsl");
const SOUL: &str = include_str!("../presets/soul.wgsl");
const FLUID: &str = include_str!("../presets/fluid.wgsl");
const LIQUID_TOY: &str = include_str!("../presets/liquid_toy.wgsl");
const PAINT_STREAMS: &str = include_str!("../presets/paint_streams.wgsl");
const DRY_ICE: &str = include_str!("../presets/dry_ice.wgsl");

pub const ALL: &[Preset] = &[
    Preset {
        name: "Plasma",
        code: PLASMA,
    },
    Preset {
        name: "Raymarch",
        code: RAYMARCH,
    },
    Preset {
        name: "Mandelbrot",
        code: MANDELBROT,
    },
    Preset {
        name: "Julia",
        code: JULIA,
    },
    Preset {
        name: "Star Nest",
        code: STAR_NEST,
    },
    Preset {
        name: "Fire",
        code: FIRE,
    },
    Preset {
        name: "Voronoi",
        code: VORONOI,
    },
    Preset {
        name: "Metaballs",
        code: METABALLS,
    },
    Preset {
        name: "Tunnel",
        code: TUNNEL,
    },
    Preset {
        name: "Menger",
        code: MENGER,
    },
    Preset {
        name: "Elevated",
        code: ELEVATED,
    },
    Preset {
        name: "Perlin",
        code: PERLIN,
    },
    Preset {
        name: "Sunset",
        code: SUNSET,
    },
    Preset {
        name: "Fire2",
        code: FIRE2,
    },
    Preset {
        name: "Seascape",
        code: SEASCAPE,
    },
    Preset {
        name: "Protean Clouds",
        code: PROTEAN_CLOUDS,
    },
    Preset {
        name: "Soul",
        code: SOUL,
    },
    Preset {
        name: "Fluid",
        code: FLUID,
    },
    Preset {
        name: "Liquid Toy",
        code: LIQUID_TOY,
    },
    Preset {
        name: "Paint Streams",
        code: PAINT_STREAMS,
    },
    Preset {
        name: "Dry Ice",
        code: DRY_ICE,
    },
];

/// The preset a fresh session starts from when "random preset" is off.
pub const DEFAULT: &Preset = &ALL[0];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_user_shader;

    /// Every preset offers knobs in the Constants window, and none of its
    /// literal constants silently fails to be live (those are marked
    /// `// tweak: off` instead).
    #[test]
    fn presets_have_live_constants() {
        for preset in ALL {
            let compiled = compile_user_shader(preset.code).expect("preset compiles");
            assert!(
                !compiled.tweaks.is_empty(),
                "{}: no live constants",
                preset.name
            );
            assert!(
                compiled.fixed.is_empty(),
                "{}: constants left fixed: {:?}",
                preset.name,
                compiled.fixed
            );
        }
    }

    #[test]
    fn all_presets_compile() {
        for preset in ALL {
            compile_user_shader(preset.code)
                .unwrap_or_else(|e| panic!("preset `{}` failed: {e}", preset.name));
        }
    }
}
