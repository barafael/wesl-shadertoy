// Voronoi — drifting cellular cells with dark seams between them.
// author: wesl-shadertoy
// license: MIT OR Apache-2.0
// tags: 2d, noise
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
