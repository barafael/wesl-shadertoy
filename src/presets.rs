//! Built-in example shaders, written in the wesl-shadertoy dialect: the
//! editor text is the `package::user` module, so each preset starts with the
//! `import package::shadertoy::...` lines that pull in the uniforms it uses.

pub struct Preset {
    pub name: &'static str,
    pub code: &'static str,
}

const PLASMA: &str = r#"// Plasma — the classic cosine palette, driven by i_time.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();
    let col = 0.5 + 0.5 * cos(t + uv.xyx + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;

const RAYMARCH: &str = r#"// Raymarch — a bobbing sphere over a checkered ground, lit and mouse-steered.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;
import package::shadertoy::i_mouse;

fn sd_sphere(p: vec3<f32>, c: vec3<f32>, r: f32) -> f32 {
    return length(p - c) - r;
}

// Returns (distance, material id).
fn map(p: vec3<f32>) -> vec2<f32> {
    let t = i_time();
    let bob = vec3<f32>(0.0, 0.4 + 0.2 * sin(t * 2.0), 0.0);
    let d_sphere = sd_sphere(p, bob, 0.8);
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
    for (var i = 0; i < 96; i = i + 1) {
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
        let sky = mix(
            vec3<f32>(0.1, 0.12, 0.2),
            vec3<f32>(0.5, 0.6, 0.8),
            clamp(rd.y + 0.4, 0.0, 1.0),
        );
        return vec4<f32>(sky, 1.0);
    }

    let n = normal(p);
    let light_dir = normalize(vec3<f32>(0.6, 0.9, -0.4));
    let diff = max(dot(n, light_dir), 0.0);
    var albedo = vec3<f32>(0.9, 0.4, 0.2);
    if (hit_id > 1.5) {
        let chk = step(0.5, fract(p.x * 2.0)) + step(0.5, fract(p.z * 2.0));
        albedo = mix(vec3<f32>(0.15), vec3<f32>(0.8), fract(chk * 0.5));
    }
    let col = albedo * (0.25 + 0.75 * diff);
    return vec4<f32>(col, 1.0);
}
"#;

const MANDELBROT: &str = r#"// Mandelbrot — breathing zoom into the seahorse valley.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let zoom = exp(sin(t * 0.3) * 5.0 + 3.0);
    let center = vec2<f32>(-0.745, 0.113);
    let c = center + (uv * 1.5) / zoom;

    var z = vec2<f32>(0.0, 0.0);
    var iters = 0.0;
    for (var k = 0; k < 128; k = k + 1) {
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        if (dot(z, z) > 4.0) {
            break;
        }
        iters = iters + 1.0;
    }

    if (iters > 127.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let f = iters / 128.0;
    let col = 0.5 + 0.5 * cos(3.0 + f * 12.0 + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;

const JULIA: &str = r#"// Julia — the Mandelbrot's sibling, with c orbiting the main cardioid.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    let t = i_time();
    let c = 0.7885 * vec2<f32>(cos(t * 0.4), sin(t * 0.4));

    var z = uv * 1.6;
    var iters = 0.0;
    for (var k = 0; k < 128; k = k + 1) {
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        if (dot(z, z) > 4.0) {
            break;
        }
        iters = iters + 1.0;
    }

    if (iters > 127.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let f = iters / 128.0;
    let col = 0.5 + 0.5 * cos(2.0 + f * 14.0 + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;

const STAR_NEST: &str = r#"// Star Nest — Dave Hoskins' iconic fractal zoom, folded and re-lit each pass.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    var p = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * 0.15;
    let c = vec2<f32>(0.7 + 0.1 * cos(t), 0.85 + 0.1 * sin(t));

    // The fold keeps p in a bounded region; each pass adds a 1/d^2 glow
    // spike, which is what makes the fractal stars blow out to white.
    var col = vec3<f32>(0.0);
    for (var k = 0; k < 8; k = k + 1) {
        p = abs(p) / clamp(dot(p, p), 0.05, 1.0) - c;
        let d = length(p);
        let glow = 0.006 / (d * d + 0.0008);
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
    let q = vec2<f32>(uv.x, uv.y - t * 0.9);
    var f = fbm(q * 3.5);
    // Hot at the bottom of the screen, dying out toward the top.
    f = f * clamp((0.5 - uv.y) * 2.0, 0.0, 1.0);
    f = clamp(f * 2.4 - 0.55, 0.0, 1.0);

    var col = mix(vec3<f32>(0.05, 0.0, 0.0), vec3<f32>(0.9, 0.15, 0.0), smoothstep(0.0, 0.5, f));
    col = mix(col, vec3<f32>(1.0, 0.6, 0.05), smoothstep(0.45, 0.75, f));
    col = mix(col, vec3<f32>(1.0, 1.0, 0.6), smoothstep(0.75, 1.0, f));
    return vec4<f32>(col, 1.0);
}
"#;

const VORONOI: &str = r#"// Voronoi — drifting cellular cells with dark seams between them.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn hash2(p: vec2<f32>) -> vec2<f32> {
    return fract(
        sin(vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3))))
            * 43758.5453,
    );
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * 0.6;

    let p = uv * 4.0;
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
        vec3<f32>(cell.x * 12.0, cell.y * 12.0, (cell.x + cell.y) * 9.0)
            + vec3<f32>(0.5, 2.0, 3.5),
    );
    let col = shade * (1.0 - 0.8 * smoothstep(0.15, 0.45, d));
    return vec4<f32>(col, 1.0);
}
"#;

const METABALLS: &str = r#"// Metaballs — implicit 1/r² blobs that fuse, with a fresnel-ish glow.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    var f = 0.0;
    for (var i = 0; i < 7; i = i + 1) {
        let a = t * (0.5 + 0.1 * f32(i)) + f32(i) * 2.3;
        let c = vec2<f32>(sin(a), cos(a * 1.31)) * 0.45;
        let d = max(dot(uv - c, uv - c), 0.0001);
        f = f + 0.02 / d;
    }

    let glow = smoothstep(0.35, 0.95, f);
    let inside = smoothstep(0.9, 1.05, f);
    let col = vec3<f32>(0.02, 0.03, 0.08)
        + vec3<f32>(0.1, 0.5, 0.9) * glow * 0.6
        + vec3<f32>(0.9, 0.98, 1.0) * inside;
    return vec4<f32>(col, 1.0);
}
"#;

const TUNNEL: &str = r#"// Tunnel — the classic infinite corridor from the 1/r polar trick.
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    let r = length(uv);
    let a = atan2(uv.y, uv.x);

    let depth = 1.0 / max(r, 0.0005);
    let z = depth + t * 4.0;
    let u = a * 4.0;

    // WESL reserves `mod` for module declarations, so the modulo is spelled out.
    let s = floor(z * 0.5) + floor(u);
    let chk = s - 2.0 * floor(s * 0.5);
    let pal = 0.5 + 0.5 * cos(z * 0.35 + vec3<f32>(0.0, 2.0, 4.0));
    var col = pal * (0.25 + 0.75 * chk);
    col = col * exp(-depth * 0.12);
    // A lamp pulsing at the far end of the tunnel.
    col = col + vec3<f32>(0.6, 0.7, 1.0) * exp(-r * 12.0) * (0.5 + 0.5 * sin(t * 3.0));
    return vec4<f32>(col, 1.0);
}
"#;

const MENGER: &str = r#"// Menger — the classic sponge fractal, raymarched with a slow spin.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

fn sd_box(p: vec3<f32>, b: f32) -> f32 {
    let q = abs(p) - vec3<f32>(b);
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

// Box minus three orthogonal square tunnels, folded at four scales.
fn sd_menger(p_in: vec3<f32>) -> f32 {
    var p = p_in;
    var s = 1.0;
    var d = sd_box(p, 1.0);
    for (var i = 0; i < 4; i = i + 1) {
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
    return sd_menger(rot_x(rot_y(p, i_time() * 0.3), 0.65));
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

    let ro = vec3<f32>(0.0, 0.0, -3.4);
    let rd = normalize(vec3<f32>(uv, 1.6));

    var t = 0.0;
    var p = ro;
    var hit = 0.0;
    for (var i = 0; i < 110; i = i + 1) {
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
        return vec4<f32>(vec3<f32>(0.02, 0.02, 0.04), 1.0);
    }

    let n = normal(p);
    let diff = max(dot(n, normalize(vec3<f32>(0.5, 0.8, -0.6))), 0.0);
    let fog = 1.0 - clamp(t / 8.0, 0.0, 1.0);
    let col = vec3<f32>(0.75, 0.78, 0.85) * (0.15 + 0.85 * diff) * fog;
    return vec4<f32>(col, 1.0);
}
"#;

const ELEVATED: &str = r#"// Elevated — TDM's classic: fly over fractal terrain, snow on the flats.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;

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
    let h = fbm(p.xz * 0.25) * 3.0 - 1.2;
    return p.y - h;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();

    let ro = vec3<f32>(0.0, 1.6 + 0.3 * sin(t * 0.4), t * 1.5);
    let rd = normalize(vec3<f32>(uv.x, uv.y - 0.25, 1.0));

    var tm = 0.0;
    var p = ro;
    var hit = 0.0;
    for (var i = 0; i < 90; i = i + 1) {
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
    let diff = max(dot(n, normalize(vec3<f32>(0.4, 0.8, 0.2))), 0.0);

    var col = mix(
        vec3<f32>(0.2, 0.35, 0.15),
        vec3<f32>(0.5, 0.45, 0.35),
        clamp(p.y * 0.4 + 0.3, 0.0, 1.0),
    );
    // Snow settles on the flats of the high ground.
    col = mix(col, vec3<f32>(0.95), smoothstep(0.6, 1.2, p.y) * step(0.7, n.y));
    col = col * (0.35 + 0.65 * diff);
    col = mix(col, sky, 1.0 - exp(-tm * tm * 0.0006));
    return vec4<f32>(col, 1.0);
}
"#;

/// Gustavson's 2D gradient noise (MIT, Stefan Gustavson / munrocket gist,
/// as shipped in the wesl-rs `random_wgsl` example), inlined into a
/// domain-warped fBm landscape.
const PERLIN: &str = r#"// Perlin — Gustavson's 2D gradient noise (MIT), rolled into fBm and
// domain-warped: the noise field steers its own sample coordinates.
import package::shadertoy::{i_time, i_resolution};

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
    for (var i = 0; i < 5; i = i + 1) {
        value = value + amp * perlin_noise2(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        amp = amp * 0.5;
    }
    return value;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time() * 0.15;
    let warp = vec2<f32>(
        fbm(uv * 2.0 + vec2<f32>(t, -t * 0.7)),
        fbm(uv * 2.0 + vec2<f32>(-t * 0.4, t)),
    );
    let f = fbm(uv * 3.0 + 1.5 * warp + vec2<f32>(0.0, t * 0.5));
    let sea = vec3<f32>(0.05, 0.12, 0.24);
    let land = vec3<f32>(0.45, 0.35, 0.22);
    let snow = vec3<f32>(0.92, 0.90, 0.85);
    var col = mix(sea, land, smoothstep(-0.15, 0.25, f));
    col = mix(col, snow, smoothstep(0.30, 0.55, f));
    col = col * (0.75 + 0.25 * smoothstep(-0.4, 0.4, warp.x + warp.y));
    return vec4<f32>(col, 1.0);
}
"#;

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
];

/// The preset a fresh session starts from when "random preset" is off.
pub const DEFAULT: &Preset = &ALL[0];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_user_shader;

    #[test]
    fn all_presets_compile() {
        for preset in ALL {
            compile_user_shader(preset.code)
                .unwrap_or_else(|e| panic!("preset `{}` failed: {e}", preset.name));
        }
    }
}
