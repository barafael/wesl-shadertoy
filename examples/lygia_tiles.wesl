// Lygia Tiles — hexagonal, triangular and square tilings, folded through a
// kaleidoscope.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, pattern
// The tilings, kaleidoscope, noise and palette come from LYGIA
// (https://lygia.xyz). Each tiling returns the position inside its tile (xy)
// and the tile's id (zw), which picks the color here.
import package::shadertoy::{i_time, i_resolution};
import lygia::space::hexTile::hexTile;
import lygia::space::triTile::triTile;
import lygia::space::sqTile::sqTile;
import lygia::space::kaleidoscope::kaleidoscope_full;
import lygia::space::rotate::rotate;
import lygia::generative::cnoise::cnoise3;
import lygia::color::palette::spectral::spectral;
import lygia::color::mixOklab::mixOklab;

// Seconds each tiling is shown before morphing into the next.
const HOLD: f32 = 4.0; // tweak: 1..12
const SEGMENTS: f32 = 6.0; // tweak: 1..16
const TILES: f32 = 6.0; // tweak: 2..20
const SPIN: f32 = 0.1; // tweak: -1..1
const GROUT: f32 = 0.06; // tweak: 0..0.3
const GROUT_COLOR: vec3<f32> = vec3<f32>(0.03, 0.025, 0.02);

/// Tiling `i` at `st`: distance from the tile's edge (0 at the grout) and a
/// hue picked from its id.
fn tiling(i: i32, st: vec2<f32>, t: f32) -> vec2<f32> {
    var tile: vec4<f32>;
    var edge: f32;
    switch i {
        case 0: {
            tile = hexTile(st);
            let p = abs(tile.xy - 0.5);
            edge = 0.5 - max(dot(p, vec2<f32>(0.5, 0.8660254)), p.x);
        }
        case 1: {
            tile = triTile(st);
            edge = min(min(tile.x, tile.y), 1.0 - tile.x - tile.y);
        }
        default: {
            tile = sqTile(st);
            let p = abs(tile.xy - 0.5);
            edge = 0.5 - max(p.x, p.y);
        }
    }
    let hue = 0.5 + 0.5 * cnoise3(vec3<f32>(tile.zw * 0.35, t));
    return vec2<f32>(edge, hue);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    // kaleidoscope_full works in 0..1 around (0.5, 0.5).
    let st = (frag_coord - 0.5 * res) / res.y + 0.5;
    let t = i_time();
    var k = kaleidoscope_full(rotate(st, SPIN * t), SEGMENTS, 0.05 * t);
    k = k * TILES + vec2<f32>(0.0, 0.3 * t);

    let phase = t / HOLD;
    let i = i32(floor(phase)) % 3;
    let blend = smoothstep(0.85, 1.0, fract(phase));
    let a = tiling(i, k, 0.1 * t);
    let b = tiling((i + 1) % 3, k, 0.1 * t);

    var col = vec3<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        let d = select(a, b, j == 1);
        let w = select(1.0 - blend, blend, j == 1);
        let fw = fwidth(d.x);
        let tint = spectral(d.y) * (0.55 + 0.45 * smoothstep(0.0, 0.5, d.x));
        col += w * mixOklab(GROUT_COLOR, tint, smoothstep(GROUT - fw, GROUT + fw, d.x));
    }
    return vec4<f32>(col, 1.0);
}
