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
];

pub const DEFAULT: &str = PLASMA;

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
