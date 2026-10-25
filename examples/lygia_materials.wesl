// Lygia Materials — a row of spheres from glossy to rough, lit with
// Cook-Torrance and Oren-Nayar, ACES-tonemapped.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 3d, raymarching, lighting
// The distance fields, camera, BRDFs, checker and tonemapper come from LYGIA
// (https://lygia.xyz); the ray marcher is ours, since LYGIA's WESL port
// leaves its `raymarch` helpers for later. Drag to orbit.
import package::shadertoy::{i_time, i_resolution, i_mouse};
import lygia::sdf::sphereSDF::sphereSDF1;
import lygia::sdf::boxSDF::boxSDF1;
import lygia::sdf::opUnion::opUnionSmooth;
import lygia::space::lookAt::lookAt3;
import lygia::space::checkerTile::checkerTile_s;
import lygia::lighting::specular::cookTorrance::specularCookTorrance;
import lygia::lighting::diffuse::orenNayar::diffuseOrenNayar;
import lygia::lighting::fresnel::fresnel;
import lygia::color::tonemap::aces::tonemapACES3;
import lygia::color::space::hsv2rgb::hsv2rgb;

const SPHERES: i32 = 5; // tweak: 1..9
const ORBIT_SPEED: f32 = 0.15; // tweak: 0..1
const EXPOSURE: f32 = 0.8; // tweak: 0.1..3
// How much the spheres melt into the plinth beneath them.
const BLEND: f32 = 0.15; // tweak: 0..0.5
const SUN_DIRECTION: vec3<f32> = vec3<f32>(0.6, 0.8, 0.4);
const SUN_COLOR: vec3<f32> = vec3<f32>(3.0, 2.7, 2.3);
const SKY_COLOR: vec3<f32> = vec3<f32>(0.06, 0.1, 0.2);
const HORIZON_COLOR: vec3<f32> = vec3<f32>(0.25, 0.27, 0.3);
const FLOOR_COLOR: vec3<f32> = vec3<f32>(0.16, 0.15, 0.14);

/// Distance to the scene, and the index of what was hit (-1: the floor).
fn map(p: vec3<f32>) -> vec2<f32> {
    let n = f32(SPHERES);
    // The sphere nearest p along x, and its slot index.
    let i = clamp(round(p.x + 0.5 * (n - 1.0)), 0.0, n - 1.0);
    let center = vec3<f32>(i - 0.5 * (n - 1.0), 0.45, 0.0);
    let sphere = sphereSDF1(p - center, 0.4);
    let plinth = boxSDF1(p, vec3<f32>(0.5 * n, 0.05, 0.5));
    let object = opUnionSmooth(sphere, plinth, BLEND);
    let ground = p.y + 0.05;
    if ground < object {
        return vec2<f32>(ground, -1.0);
    }
    return vec2<f32>(object, i);
}

fn normal(p: vec3<f32>) -> vec3<f32> {
    let e = vec2<f32>(0.001, 0.0);
    return normalize(vec3<f32>(
        map(p + e.xyy).x - map(p - e.xyy).x,
        map(p + e.yxy).x - map(p - e.yxy).x,
        map(p + e.yyx).x - map(p - e.yyx).x,
    ));
}

/// Soft shadow towards the sun.
fn shadow(p: vec3<f32>, l: vec3<f32>) -> f32 {
    var lit = 1.0;
    var t = 0.02;
    for (var i = 0; i < 48; i++) {
        let d = map(p + l * t).x;
        lit = min(lit, 12.0 * d / t);
        t += clamp(d, 0.01, 0.3);
        if lit < 0.001 || t > 8.0 {
            break;
        }
    }
    return clamp(lit, 0.0, 1.0);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;

    // Orbit on its own, or follow a mouse drag.
    var angle = ORBIT_SPEED * i_time();
    var height = 1.6;
    let mouse = i_mouse();
    if mouse.z > 0.0 {
        angle = 6.2831853 * (mouse.x / res.x - 0.5);
        height = mix(0.3, 4.0, mouse.y / res.y);
    }
    let distance = 1.2 * f32(SPHERES) + 1.5;
    let eye = vec3<f32>(distance * sin(angle), height, distance * cos(angle));
    let target_ = vec3<f32>(0.0, 0.35, 0.0);
    let rd = lookAt3(eye, target_, vec3<f32>(0.0, 1.0, 0.0)) * normalize(vec3<f32>(uv, 1.8));

    var t = 0.0;
    var hit = vec2<f32>(0.0, -2.0);
    for (var i = 0; i < 128; i++) {
        let h = map(eye + rd * t);
        if h.x < 0.0005 * t {
            hit = h;
            break;
        }
        t += h.x;
        if t > 40.0 {
            break;
        }
    }

    var col = mix(SKY_COLOR, HORIZON_COLOR, pow(1.0 - max(rd.y, 0.0), 6.0));
    if hit.y > -2.0 {
        let p = eye + rd * t;
        let n = normal(p);
        let v = -rd;
        let l = normalize(SUN_DIRECTION);
        let h = normalize(l + v);
        let nov = max(dot(n, v), 1e-4);
        let nol = max(dot(n, l), 0.0);
        let noh = max(dot(n, h), 0.0);

        // Oren-Nayar already includes the cosine term; Cook-Torrance does not.
        // Spheres go from glossy (left) to rough (right), each its own hue.
        var albedo = FLOOR_COLOR * (0.6 + 0.4 * checkerTile_s(p.xz + 64.0, 2.0));
        var roughness = 0.8;
        if hit.y >= 0.0 {
            let k = hit.y / max(f32(SPHERES) - 1.0, 1.0);
            albedo = hsv2rgb(vec3<f32>(fract(0.08 + 0.6 * k), 0.85, 0.55));
            roughness = mix(0.08, 0.9, k);
        }
        let f0 = vec3<f32>(0.04);
        let lit = shadow(p + n * 0.002, l);
        let diffuse = albedo * diffuseOrenNayar(l, n, v, nov, nol, roughness);
        let specular = specularCookTorrance(l, n, h, nov, nol, noh, roughness, f0);
        let ambient = 2.0 * SKY_COLOR * albedo * (0.5 + 0.5 * n.y)
            + 0.3 * SKY_COLOR * fresnel(f0, nov) * (1.0 - roughness);
        col = SUN_COLOR * lit * (diffuse + specular * nol) + ambient;
        col = mix(col, HORIZON_COLOR, 1.0 - exp(-0.001 * t * t));
    }
    return vec4<f32>(tonemapACES3(col * EXPOSURE), 1.0);
}
