// Lygia Pinwheels — brick, windmill and mirror tilings, drawn as bricks,
// quilt pinwheels and folded stripes.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, pattern
// Three panels, one per LYGIA tiling in `space` (https://lygia.xyz). Each
// returns the position inside a tile (xy, 0..1) and the tile's id (zw):
//   brickTile    offsets every other row by half a tile: a brick wall.
//   windmillTile turns each tile of a 2×2 block a quarter further, so one
//                half-square triangle per tile becomes a field of pinwheels.
//   mirrorTile   flips every other tile, so a diagonal becomes a chevron.
import package::shadertoy::{i_time, i_resolution};
import lygia::space::brickTile::brickTile_s2;
import lygia::space::windmillTile::windmillTile_s;
import lygia::space::mirrorTile::mirrorTile_s;
import lygia::generative::random::random2;
import lygia::color::mixOklab::mixOklab;
import lygia::color::space::hsv2rgb::hsv2rgb;

const TILES: f32 = 6.0; // tweak: 2..20
const SCROLL: f32 = 0.15; // tweak: -1..1
const STRIPES: f32 = 4.0; // tweak: 1..12
const MORTAR_COLOR: vec3<f32> = vec3<f32>(0.75, 0.7, 0.62);
const BRICK_COLOR: vec3<f32> = vec3<f32>(0.45, 0.13, 0.07);
const INK_COLOR: vec3<f32> = vec3<f32>(0.02, 0.025, 0.04);
const PAPER_COLOR: vec3<f32> = vec3<f32>(0.9, 0.85, 0.72);
const GAP_COLOR: vec3<f32> = vec3<f32>(0.016, 0.015, 0.014);

fn aa(d: f32, px: f32) -> f32 {
    return smoothstep(-px, px, d);
}

fn bricks(st: vec2<f32>, px: f32) -> vec3<f32> {
    // Bricks twice as wide as tall.
    let t = brickTile_s2(st, vec2<f32>(0.5 * TILES, TILES));
    let edge = min(min(t.x, 1.0 - t.x) * 2.0, min(t.y, 1.0 - t.y));
    let shade = 0.7 + 0.6 * random2(t.zw);
    return mix(MORTAR_COLOR, BRICK_COLOR * shade, aa(edge - 0.08, px));
}

fn pinwheels(st: vec2<f32>, px: f32) -> vec3<f32> {
    let t = windmillTile_s(st, TILES);
    // A half-square triangle per tile: turned a quarter further in each tile
    // of a 2×2 block, four of them make the quilter's pinwheel.
    let blade = aa(t.x + t.y - 1.0, px);
    let block = floor(t.zw / 2.0);
    let tint = hsv2rgb(vec3<f32>(fract(0.13 * (block.x + 3.0 * block.y)), 0.5, 0.85));
    return mix(mixOklab(PAPER_COLOR, tint, 0.35), mixOklab(INK_COLOR, tint, 0.4), blade);
}

fn chevrons(st: vec2<f32>, px: f32) -> vec3<f32> {
    let t = mirrorTile_s(st, TILES);
    let s = fract((t.x + t.y) * STRIPES);
    let band = aa(abs(s - 0.5) - 0.25, px * STRIPES);
    let tint = hsv2rgb(vec3<f32>(fract(0.55 + 0.04 * (t.z + t.w)), 0.6, 0.8));
    return mix(tint, PAPER_COLOR, band);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let w = res.x / 3.0;
    let panel = min(floor(frag_coord.x / w), 2.0);
    // The tilings use % on the tile ids, which flips sign below zero:
    // keep the coordinates positive.
    let st = frag_coord / res.y + vec2<f32>(SCROLL * i_time(), 0.5 * SCROLL * i_time()) + 100.0;
    // One pixel in tile units, for antialiasing.
    let px = TILES / res.y;
    var col: vec3<f32>;
    switch i32(panel) {
        case 0: { col = bricks(st, px); }
        case 1: { col = pinwheels(st, px); }
        default: { col = chevrons(st, px); }
    }
    let border = abs(frag_coord.x - w * round(frag_coord.x / w));
    col = mix(GAP_COLOR, col, step(2.0, border));
    return vec4<f32>(col, 1.0);
}
