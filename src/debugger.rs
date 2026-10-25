//! In-shader debugger overlay (MDE-style), ported from `eliotbo/bevy_shadertoy_wgsl`
//! (`src/templates/debugger.wgsl`, dual-licensed MIT OR Apache-2.0).
//!
//! The reference injected the overlay into the user's image shader and read
//! the rendered image from its storage texture. Here it is an engine-owned
//! post-pass instead. While [`DebuggerOn`] is set, the main pass renders into
//! an offscreen [`ImageTarget`] and [`debugger_blit_pass`] draws it to the
//! view through [`DEBUGGER_WGSL`]'s `fragment_blit` entry, with the overlay
//! (mouse position, resolution, fps, RGBA probe boxes, crosshair and click
//! ring) on top. While it is off, the main pass renders straight to the view
//! and the debugger costs nothing: no extra pass, and a 1×1 target.
//!
//! Coordinate note: the reference computed the overlay in texel space and
//! flipped the whole image on display, which left its y-down `iMouse`
//! vertically mirrored against the cursor. This port works directly in
//! y-up display space (`frag_coord` and `i_mouse` both bottom-left origin),
//! so the crosshair, ring and probe boxes track the cursor exactly.
//!
//! The 16×16 glyph atlas is rasterized at startup from egui's embedded Hack
//! TTF with `ab_glyph` ([`font_atlas_pixels`]) instead of shipping the
//! reference's 1.3 MB `font.png`.

use bevy::asset::Handle;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::SystemParam;
use bevy::math::UVec2;
use bevy::render::RenderApp;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::binding_types::{texture_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, CachedRenderPipelineId, ColorTargetState,
    ColorWrites, Extent3d, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, ShaderStages, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor,
};
use bevy::render::render_resource::{BindGroupLayoutEntries, Texture};
use bevy::render::renderer::{RenderAdapter, RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::shader::Shader;
use bevy::{
    asset::Assets,
    ecs::system::{Commands, Query, Res, ResMut},
    render::renderer::RenderQueue,
};

use crate::render::ShaderToyUniform;
use crate::render::fullscreen_material::{
    FullscreenMaterialPipeline, FullscreenMaterialPipelineId,
};

/// Preferred format of the offscreen target the main pass renders into.
/// `Rgba16Float` stores the user's written values raw (no 8-bit round trip),
/// so the probe boxes show exactly what the shader wrote and the blit's write
/// into the sRGB view target encodes them exactly once.
pub const IMAGE_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Stand-in where [`IMAGE_FORMAT`] is not renderable (WebGL2 without
/// `EXT_color_buffer_float`). Values are clamped to 0..1 and quantized, but
/// sRGB encoding keeps the darks precise.
const FALLBACK_IMAGE_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// Atlas geometry: 16×16 cells of 64 px, glyph `ch` at cell `(ch % 16, ch / 16)`
/// with the glyph inked in the cell's left half — the layout `chara()` in
/// [`DEBUGGER_WGSL`] samples.
pub const ATLAS_SIZE: u32 = 1024;
const CELL: u32 = 64;

/// Rasterize ASCII 0..255 (plus `×` in cell 28, the reference's resolution
/// separator) into an `ATLAS_SIZE`² single-channel coverage buffer.
pub fn font_atlas_pixels() -> Vec<u8> {
    use ab_glyph::{Font as _, FontArc, PxScale, point};

    let mut atlas = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize];
    let fonts = bevy_egui::egui::FontDefinitions::default();
    let hack = fonts
        .font_data
        .get("Hack")
        .expect("egui's default fonts include Hack");
    let font = FontArc::try_from_vec(hack.font.to_vec()).expect("Hack is a valid font");

    // Scale so the advance (~0.554 em) fits the 32 px sampling window, and
    // put the baseline 13 px above the cell bottom: ascenders reach ~56 px,
    // descenders stay inside, and both cell edges keep a blank margin so the
    // shader's 5-tap smoothing never bleeds into a neighbouring glyph.
    let scale = PxScale::from(52.0);
    let upem = font.units_per_em().unwrap_or(1000.0);
    let baseline = (CELL - 13) as f32;
    for ch in 0..256u32 {
        // Cell 28 is a control character in ASCII; the reference atlas draws
        // a multiplication sign there ("1920×1080").
        let cp = if ch == 28 {
            '\u{00D7}'
        } else {
            char::from_u32(ch).unwrap_or(' ')
        };
        let id = font.glyph_id(cp);
        if id.0 == 0 {
            continue; // .notdef — control characters and missing glyphs
        }
        let advance = font.h_advance_unscaled(id) * (scale.x / upem);
        let x = (0.5 * CELL as f32 - advance) * 0.5;
        let Some(outlined) =
            font.outline_glyph(id.with_scale_and_position(scale, point(x, baseline)))
        else {
            continue; // space and other outline-less glyphs
        };
        let bounds = outlined.px_bounds();
        let (col, row) = ((ch % 16) as i32, (ch / 16) as i32);
        let (bx, by) = (bounds.min.x.floor() as i32, bounds.min.y.floor() as i32);
        outlined.draw(|dx, dy, coverage| {
            // Clip to the cell: ink spilling past the left half (wide
            // glyphs) is never sampled by the shader anyway.
            let (px, py) = (bx + dx as i32, by + dy as i32);
            if px < 0 || px >= CELL as i32 || py < 0 || py >= CELL as i32 {
                return;
            }
            let at =
                ((row * CELL as i32 + py) * ATLAS_SIZE as i32 + col * CELL as i32 + px) as usize;
            atlas[at] = atlas[at].max((coverage.clamp(0.0, 1.0) * 255.0) as u8);
        });
    }
    atlas
}

/// The debugger post-pass shader. Entry point [`BLIT_ENTRY`] samples the main
/// pass result at `@group(0) @binding(0)`, the font atlas at binding 1 and
/// the shared [`ShaderToyUniform`] dynamic buffer at binding 2 (the same
/// buffer the user shader sees; the struct layout is mirrored here).
pub const BLIT_ENTRY: &str = "fragment_blit";

pub const DEBUGGER_WGSL: &str = concat!(
    r#"// wesl-shadertoy debugger overlay, ported from
// eliotbo/bevy_shadertoy_wgsl (src/templates/debugger.wgsl, MIT OR Apache-2.0).
// Copies the main-pass image to the view and composites the overlay (mouse
// position, resolution, fps, RGBA probe boxes, crosshair and click ring) on
// top.

struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

"#,
    crate::compile::shadertoy_uniforms_wgsl!(),
    r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var font_texture: texture_2d<f32>;
@group(0) @binding(2) var<uniform> uni: ShaderToyUniforms;

// ---- text engine (verbatim from the reference, minus dead code) -----------

const FONT_SPACE_DEBUG: f32 = 0.5;

var<private> uv_debug: vec2<f32>;
var<private> tp_debug: vec2<f32>;
var<private> align_debug: vec4<f32>; // north, east, south, west
var<private> drawColorDebug: vec3<f32> = vec3<f32>(1., 1., 0.);
var<private> vColor: vec3<f32> = vec3<f32>(0.2, 0.2, 0.2);
var<private> text_unit: f32 = 18.; // px per text unit (one glyph is 0.5 x 1)

const mxColorDebug: vec3<f32> = vec3<f32>(1., 0.45, 0.4);
const myColorDebug: vec3<f32> = vec3<f32>(0.5, 1., 0.45);
const headColorDebug: vec3<f32> = vec3<f32>(0.9, 0.6, 0.2);

// Glyph coverage for `ch` at the text cursor. The atlas is a 16x16 grid of
// 64 px cells (glyph `ch` at column ch % 16, row ch / 16 from the top) with
// the ink in the cell's left half; the cursor advances 0.5 text units per
// character, so a glyph is sampled where tp_debug is in [0, 0.5] x [0, 1].
fn chara(ch: i32) -> f32 {
    let fr = fract(floor(vec2<f32>(f32(ch), 15.999 - f32(ch) / 16.)) / 16.);
    let q = clamp(tp_debug, vec2<f32>(0.), vec2<f32>(1.)) / 16. + fr;
    let q1 = vec2<i32>(1023. * q);
    let yq = vec2<i32>(q1.x, 1023 - q1.y);

    var f: f32 = textureLoad(font_texture, yq, 0).r;
    // smoothing out the aliasing
    let dx = vec2<i32>(1, 0);
    let dy = vec2<i32>(0, 1);
    let fx1: f32 = textureLoad(font_texture, yq + dx, 0).r;
    let fx2: f32 = textureLoad(font_texture, yq - dx, 0).r;
    let fy1: f32 = textureLoad(font_texture, yq + dy, 0).r;
    let fy2: f32 = textureLoad(font_texture, yq - dy, 0).r;
    let rp = 0.25;
    f = f * rp + (1.0 - rp) * (fx1 + fx2 + fy1 + fy2) / 4.0;
    return f;
}

fn SetTextPositionAbs(x: f32, y: f32) {
    tp_debug.x = 10. * uv_debug.x - x;
    tp_debug.y = 10. * uv_debug.y - y;
}

fn drawFract(value: ptr<function, f32>, digits: ptr<function, i32>) -> f32 {
    var c: f32 = 0.;
    *value = fract(*value) * 10.;

    for (var ni: i32 = 1; ni < 60; ni = ni + 1) {
        c = c + (chara(48 + i32(*value)));
        tp_debug.x = tp_debug.x - (0.5);
        *digits = *digits - (1);
        *value = fract(*value) * 10.;

        if (*digits <= 0 || *value == 0.) {
            break;
        }
    }

    tp_debug.x = tp_debug.x - (0.5 * f32(*digits));
    return c;
}

fn maxInt(a: i32, b: i32) -> i32 {
    var ret: i32;
    if (a > b) { ret = a; } else { ret = b; };
    return ret;
}

fn drawInt(value: ptr<function, i32>, minDigits: ptr<function, i32>) -> f32 {
    var c: f32 = 0.;
    if (*value < 0) {
        *value = -*value;
        if (*minDigits < 1) {
            *minDigits = 1;
        } else {
            *minDigits = *minDigits - 1;
        }
        c = c + (chara(45));
        tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
    }
    var fn2: i32 = *value;
    var digits: i32 = 1;

    for (var ni: i32 = 0; ni < 10; ni = ni + 1) {
        fn2 = fn2 / (10);
        if (fn2 == 0) { break; }
        digits = digits + 1;
    }

    digits = maxInt(*minDigits, digits);
    tp_debug.x = tp_debug.x - (0.5 * f32(digits));

    for (var ni: i32 = 1; ni < 11; ni = ni + 1) {
        tp_debug.x = tp_debug.x + (0.5);
        c = c + (chara(48 + *value % 10));
        *value = *value / (10);
        if (ni >= digits) { break; }
    }

    tp_debug.x = tp_debug.x - (0.5 * f32(digits));
    return c;
}

fn drawIntBackwards(value: ptr<function, i32>, minDigits: ptr<function, i32>) -> f32 {
    var c: f32 = 0.;
    let original_value: i32 = *value;

    if (*value < 0) {
        *value = -*value;
        if (*minDigits < 1) {
            *minDigits = 1;
        } else {
            *minDigits = *minDigits - 1;
        }
    }
    var fn2: i32 = *value;
    var digits: i32 = 1;

    for (var ni: i32 = 0; ni < 10; ni = ni + 1) {
        fn2 = fn2 / (10);
        if (fn2 == 0) { break; }
        digits = digits + 1;
    }

    digits = maxInt(*minDigits, digits);

    for (var ni: i32 = digits - 1; ni < 11; ni = ni - 1) {
        tp_debug.x = tp_debug.x + (0.5);
        c = c + (chara(48 + *value % 10));
        *value = *value / (10);
        if (ni == 0) { break; }
    }

    if (original_value < 0) {
        tp_debug.x = tp_debug.x + (FONT_SPACE_DEBUG);
        c = c + (chara(45));
    }

    return c;
}

fn drawFloat(val: f32, prec: ptr<function, i32>, maxDigits: i32) -> f32 {
    // in case of 0.099999..., round up to 0.1000000
    var value = round(val * pow(10., f32(maxDigits))) / pow(10., f32(maxDigits));

    let tp_debugx: f32 = tp_debug.x - 0.5 * f32(maxDigits);
    var c: f32 = 0.;
    if (value < 0.) {
        c = chara(45);
        value = -value;
    }
    tp_debug.x = tp_debug.x - (0.5);
    var ival = i32(value);

    var one: i32 = 1;

    c = c + (drawInt(&ival, &one));
    c = c + (chara(46));
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);

    var frac_val = fract(value);
    c = c + (drawFract(&frac_val, prec));
    tp_debug.x = min(tp_debug.x, tp_debugx);

    return c;
}

fn SetColor(red: f32, green: f32, blue: f32) {
    drawColorDebug = vec3<f32>(red, green, blue);
}

fn WriteFloat(fValue: f32, maxDigits: i32, decimalPlaces: ptr<function, i32>) {
    vColor = mix(vColor, drawColorDebug, drawFloat(fValue, decimalPlaces, maxDigits));
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
}

fn WriteFloatBox(fValue: f32, maxDigits: i32, decimalPlaces: i32, alpha: f32) {
    var decs = decimalPlaces;
    vColor = mix(vColor, drawColorDebug, drawFloat(fValue, &decs, maxDigits) * alpha);
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
}

fn WriteIntegerBack(iValue: ptr<function, i32>) {
    var one: i32 = 1;
    vColor = mix(vColor, drawColorDebug, drawIntBackwards(iValue, &one));
    tp_debug.x = tp_debug.x + (FONT_SPACE_DEBUG);
}

fn WriteFPS() {
    let fps: f32 = uni.i_frame_rate;
    SetColor(0.8, 0.6, 0.3);
    var max_digits_one = 1;
    WriteFloat(fps, 5, &max_digits_one);
    var c: f32 = 0.;
    c = c + (chara(102)); // f
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
    c = c + (chara(112)); // p
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
    c = c + (chara(115)); // s
    tp_debug.x = tp_debug.x - (FONT_SPACE_DEBUG);
    vColor = mix(vColor, drawColorDebug, c);
}

// `x_pos` is the right edge of the y value; the x value ends 7 glyphs further
// left, so either number fits up to 7 characters.
fn WriteMousePos(mPos: vec2<f32>, x_pos: f32, y_pos: f32) {
    var mposxi: i32 = i32(mPos.x);
    var mposyi: i32 = i32(mPos.y);

    SetTextPositionAbs(x_pos, y_pos);
    drawColorDebug = myColorDebug;
    WriteIntegerBack(&mposyi);

    SetTextPositionAbs(x_pos - 7. * FONT_SPACE_DEBUG, y_pos);
    drawColorDebug = mxColorDebug;
    WriteIntegerBack(&mposxi);
}

// Width of an integer in glyphs, minus sign included.
fn glyph_count(v: i32) -> f32 {
    var n = 1.;
    var rest = abs(v) / 10;
    for (var i = 0; i < 10 && rest > 0; i = i + 1) {
        n = n + 1.;
        rest = rest / 10;
    }
    return n + select(0., 1., v < 0);
}

// ---- signed distance helpers ----------------------------------------------

fn sdRoundedBox(p: vec2<f32>, b: vec2<f32>, r: vec4<f32>) -> f32 {
    var x = r.x;
    var y = r.y;
    x = select(r.z, r.x, p.x > 0.);
    y = select(r.w, r.y, p.x > 0.);
    x = select(y, x, p.y > 0.);
    let q = abs(p) - b + x;
    return min(max(q.x, q.y), 0.) + length(max(q, vec2<f32>(0.))) - x;
}

fn sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0., 1.);
    return length(pa - ba * h);
}

fn sdCircle(p: vec2<f32>, c: vec2<f32>, r: f32) -> f32 {
    return length(p - c) - r;
}

// ---- overlay pieces ---------------------------------------------------------
//
// Layout is in text units (`text_pos()`), origin at the viewport centre, one
// glyph is 0.5 x 1 units. Everything keeps MARGIN units from the edges of the
// visible canvas (the viewport minus the toolbar and status bar rows).

const MARGIN: f32 = 0.5;
const PANEL_COLOR: vec3<f32> = vec3<f32>(0.08, 0.08, 0.1);

fn text_pos() -> vec2<f32> {
    return 10. * uv_debug;
}

// Translucent rounded rectangle between `lo` and `hi` (text units), drawn
// under readouts so they stay legible on any image.
fn panel(lo: vec2<f32>, hi: vec2<f32>, alpha: f32) {
    let d = sdRoundedBox(text_pos() - 0.5 * (lo + hi), 0.5 * (hi - lo), vec4<f32>(0.3));
    let aa = 0.5 / text_unit; // half a pixel in text units
    vColor = mix(vColor, PANEL_COLOR, (1. - smoothstep(-aa, aa, d)) * alpha);
}

// One glyph in its own colour, advancing the cursor.
fn write_char(ch: i32, color: vec3<f32>, alpha: f32) {
    vColor = mix(vColor, color, chara(ch) * alpha);
    tp_debug.x = tp_debug.x - FONT_SPACE_DEBUG;
}

// One probe box listing a colour as r/g/b/a lines; `top_left` in text units.
fn WriteRGBAValues(value: vec4<f32>, top_left: vec2<f32>, alpha: f32) {
    let size = vec2<f32>(5.5, 4.6); // keep in sync with box_width below
    panel(top_left - vec2<f32>(0., size.y), top_left + vec2<f32>(size.x, 0.), 0.85 * alpha);

    let x0 = top_left.x + 0.4;
    let y0 = top_left.y - 1.3;
    let labels = array<i32, 4>(114, 103, 98, 97); // r g b a
    let colors = array<vec3<f32>, 4>(
        vec3<f32>(1., 0.45, 0.4),
        vec3<f32>(0.5, 1., 0.45),
        vec3<f32>(0.5, 0.7, 1.),
        vec3<f32>(0.85, 0.85, 0.85),
    );
    for (var i = 0; i < 4; i = i + 1) {
        SetTextPositionAbs(x0, y0 - f32(i));
        write_char(labels[i], colors[i], alpha);
        write_char(58, colors[i], alpha); // :
        SetColor(1., 1., 1.);
        WriteFloatBox(value[i], 3, 3, alpha);
    }
}

// The image texel under a y-up pixel position, clamped to the image (the
// texture stores row 0 at the top).
fn probe(pos: vec2<f32>) -> vec4<f32> {
    let res = uni.i_resolution.xy;
    let texel = clamp(vec2<f32>(pos.x, res.y - 1. - pos.y), vec2<f32>(0.), res - 1.);
    return textureLoad(image, vec2<i32>(texel), 0);
}

fn draw_ring(location: vec2<f32>) {
    let mouse_click_poz = vec2<f32>(abs(uni.i_mouse.z), abs(uni.i_mouse.w));

    let alpha = 0.75;
    let ring_dist = sdCircle(location, mouse_click_poz, 2.3);
    let d = smoothstep(0.5, 1.5, abs(ring_dist - 1.));
    vColor = mix(vColor, headColorDebug, (1. - d) * alpha);
}

fn draw_crossair(location: vec2<f32>) {
    let start = 5.0;
    let end = 20.;
    let segment1 = sdSegment(location - uni.i_mouse.xy, vec2<f32>(start, 0.), vec2<f32>(end, 0.));
    let segment2 = sdSegment(location - uni.i_mouse.xy, vec2<f32>(-start, 0.), vec2<f32>(-end, 0.));
    let segment3 = sdSegment(location - uni.i_mouse.xy, vec2<f32>(0., start), vec2<f32>(0., end));
    let segment4 = sdSegment(location - uni.i_mouse.xy, vec2<f32>(0., -start), vec2<f32>(0., -end));

    var alpha = 0.75;
    if (uni.i_mouse.z > 0.) {
        alpha = 1.0;
    }

    let d1 = smoothstep(0.5, 1.5, segment1);
    vColor = mix(vColor, headColorDebug, (1.0 - d1) * alpha);
    let d2 = smoothstep(0.5, 1.5, segment2);
    vColor = mix(vColor, headColorDebug, (1.0 - d2) * alpha);
    let d3 = smoothstep(0.5, 1.5, segment3);
    vColor = mix(vColor, headColorDebug, (1.0 - d3) * alpha);
    let d4 = smoothstep(0.5, 1.5, segment4);
    vColor = mix(vColor, headColorDebug, (1.0 - d4) * alpha);
}

/// `frag_coord` is shadertoy's y-up pixel position; `color` the image pixel.
fn show_debug_info(frag_coord: vec2<f32>, color: vec3<f32>) -> vec3<f32> {
    vColor = color;
    let res = uni.i_resolution.xy;

    // The reference's res.y / 20 for small viewports, growing sublinearly
    // beyond 360 px so text stays a readout, not a headline.
    text_unit = min(res.y / 20., 18. * sqrt(res.y / 360.));
    // SetTextPosition* work in 10 * uv_debug.
    uv_debug = (frag_coord - 0.5 * res) / (10. * text_unit);

    let half = 0.5 * res / text_unit;
    align_debug = vec4<f32>(
        half.y,   // North
        half.x,   // East
        -half.y,  // South
        -half.x,  // West
    );
    let north = align_debug.x - uni._overlay_top / text_unit - MARGIN;
    let east = align_debug.y - MARGIN;
    let south = align_debug.z + uni._overlay_bottom / text_unit + MARGIN;
    let west = align_debug.w + MARGIN;

    // Top right: resolution, "640×360".
    var resx = i32(res.x);
    var resy = i32(res.y);
    let res_width = 0.5 * (glyph_count(resx) + 1. + glyph_count(resy));
    panel(vec2<f32>(east - res_width - 0.3, north - 1.15), vec2<f32>(east + 0.3, north + 0.15), 0.75);
    SetTextPositionAbs(east, north - 1.);
    SetColor(0.8, 0.8, 0.8);
    // Backwards drawing advances the cursor leftwards from the anchor, so
    // the number written last ends up leftmost: draw resy first to read
    // "640×360" (the reference wrote resx first and displayed "360×640").
    WriteIntegerBack(&resy);
    var c: f32 = chara(28); // the multiplication sign from the atlas
    vColor = mix(vColor, drawColorDebug, c);
    WriteIntegerBack(&resx);

    // Bottom right: current mouse position, and the click position above it
    // (negative while no button is held, like iMouse.zw).
    let x_width = 0.5 * max(glyph_count(i32(uni.i_mouse.x)), glyph_count(i32(uni.i_mouse.z)));
    panel(vec2<f32>(east - 3.5 - x_width - 0.3, south - 0.15), vec2<f32>(east + 0.3, south + 2.15), 0.75);
    WriteMousePos(uni.i_mouse.zw, east, south + 1.);
    WriteMousePos(uni.i_mouse.xy, east, south);

    // Bottom left: frame rate, "60.0 fps".
    let fps_width = 0.5 * (glyph_count(i32(uni.i_frame_rate)) + 6.);
    panel(vec2<f32>(west - 0.3, south - 0.15), vec2<f32>(west + fps_width + 0.3, south + 1.15), 0.75);
    SetTextPositionAbs(west - FONT_SPACE_DEBUG, south);
    WriteFPS();

    // Right, under the resolution: probe boxes for the pixel under the last
    // click and the pixel under the cursor (in the order of the readouts at
    // the bottom right). The left side is where the code window opens.
    let box_width = 5.5;
    let box_top = north - 1.15 - MARGIN;
    WriteRGBAValues(probe(abs(uni.i_mouse.zw)), vec2<f32>(east - box_width, box_top), 0.85);
    WriteRGBAValues(probe(uni.i_mouse.xy), vec2<f32>(east - box_width, box_top - 4.6 - MARGIN), 1.0);

    // Right centre (below the probe boxes if they reach that far): a swatch
    // of just the clicked pixel's colour, on a dark frame.
    let swatch = 2.5;
    let boxes_bottom = box_top - 2. * (4.6 + MARGIN);
    let mid = min(0.5 * (north + south), boxes_bottom - 0.5 * swatch - 0.3);
    let lo = vec2<f32>(east - swatch, mid - 0.5 * swatch);
    let hi = vec2<f32>(east, mid + 0.5 * swatch);
    panel(lo - vec2<f32>(0.3), hi + vec2<f32>(0.3), 0.85);
    let d = sdRoundedBox(text_pos() - 0.5 * (lo + hi), 0.5 * (hi - lo), vec4<f32>(0.15));
    let aa = 0.5 / text_unit;
    vColor = mix(vColor, probe(abs(uni.i_mouse.zw)).rgb, 1. - smoothstep(-aa, aa, d));

    draw_crossair(frag_coord);
    draw_ring(frag_coord);

    return vColor;
}

@fragment
fn fragment_blit(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let value = textureLoad(image, vec2<i32>(in.position.xy), 0);
    let res = uni.i_resolution.xy;
    let frag_coord = vec2<f32>(in.position.x, res.y - in.position.y);
    return vec4<f32>(show_debug_info(frag_coord, value.rgb), value.a);
}
"#
);

// ---------------------------------------------------------------------------
// Render-world plumbing
// ---------------------------------------------------------------------------

/// Whether the overlay is on: the toolbar bug icon or `--debugger`. Written
/// by the main world each frame, extracted to the render world, where it
/// decides where the main pass renders and whether the blit pass runs.
#[derive(Resource, ExtractResource, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[extract_app(RenderApp)]
pub struct DebuggerOn(pub bool);

/// Offscreen target the main fullscreen pass renders into while the
/// debugger is on, so the blit pass can probe and overlay the finished
/// image. Viewport-sized while on, 1×1 while off.
#[derive(Resource)]
pub struct ImageTarget {
    /// Kept alongside `view` to make the ownership explicit (wgpu refcounts
    /// the texture from the view, so this handle is otherwise unread).
    #[allow(dead_code)]
    texture: Texture,
    pub view: TextureView,
    pub size: UVec2,
    /// [`IMAGE_FORMAT`], or [`FALLBACK_IMAGE_FORMAT`] where that is not
    /// renderable.
    pub format: TextureFormat,
}

impl ImageTarget {
    fn new(device: &RenderDevice, size: UVec2, format: TextureFormat) -> Self {
        let size = size.max(UVec2::ONE);
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("shadertoy_image_target"),
            size: Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size,
            format,
        }
    }

    fn resize(&mut self, size: UVec2, device: &RenderDevice) {
        if size.max(UVec2::ONE) != self.size {
            *self = Self::new(device, size, self.format);
        }
    }
}

/// Where the main fullscreen pass renders: the debugger's [`ImageTarget`]
/// while the overlay is on, the view target otherwise.
#[derive(SystemParam)]
pub struct MainPassTarget<'w> {
    on: Res<'w, DebuggerOn>,
    image: Res<'w, ImageTarget>,
}

impl MainPassTarget<'_> {
    pub fn format(&self, view_format: TextureFormat) -> TextureFormat {
        if self.on.0 {
            self.image.format
        } else {
            view_format
        }
    }

    pub fn view<'a>(&'a self, view_destination: &'a TextureView) -> &'a TextureView {
        if self.on.0 {
            &self.image.view
        } else {
            view_destination
        }
    }
}

/// Static debugger resources living in the render world.
#[derive(Resource)]
pub struct Debugger {
    font_view: TextureView,
    layout: BindGroupLayoutDescriptor,
    /// Queued blit pipeline, keyed by the view target format.
    pipeline: Option<(TextureFormat, CachedRenderPipelineId)>,
}

/// Handle of the blit/debugger shader, created in the main world (the render
/// world has no `Assets<Shader>` until the first extraction) and extracted to
/// the render world for [`debugger_blit_pass`].
#[derive(Resource, ExtractResource, Clone, Debug, Default)]
#[extract_app(RenderApp)]
pub struct DebuggerShader(pub Handle<Shader>);

/// Main-world startup: register the debugger shader as an asset so Bevy's
/// regular shader extraction makes it resolvable by the render-world pipeline
/// cache.
pub fn setup_debugger_shader(mut commands: Commands, mut shaders: ResMut<Assets<Shader>>) {
    let handle = shaders.add(Shader::from_wgsl(DEBUGGER_WGSL, "shadertoy://debugger"));
    commands.insert_resource(DebuggerShader(handle));
}

/// Create the font atlas texture, the blit bind group layout and the (1×1
/// until the debugger is switched on) image target.
pub fn setup_debugger(
    mut commands: Commands,
    adapter: Res<RenderAdapter>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let pixels = font_atlas_pixels();
    let font = device.create_texture(&TextureDescriptor {
        label: Some("debugger_font_atlas"),
        size: Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::R8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        font.as_image_copy(),
        &pixels,
        bevy::render::render_resource::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ATLAS_SIZE), // R8, already a multiple of 256
            rows_per_image: Some(ATLAS_SIZE),
        },
        Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            depth_or_array_layers: 1,
        },
    );
    let font_view = font.create_view(&TextureViewDescriptor::default());

    let layout = BindGroupLayoutDescriptor::new(
        "debugger_blit_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                // The main-pass image; only ever read with textureLoad.
                texture_2d(TextureSampleType::Float { filterable: false }),
                // Font atlas coverage; only ever read with textureLoad.
                texture_2d(TextureSampleType::Float { filterable: false }),
                uniform_buffer::<ShaderToyUniform>(true),
            ),
        ),
    );

    let renderable = adapter
        .get_texture_format_features(IMAGE_FORMAT)
        .allowed_usages
        .contains(TextureUsages::RENDER_ATTACHMENT);
    let format = if renderable {
        IMAGE_FORMAT
    } else {
        FALLBACK_IMAGE_FORMAT
    };
    commands.insert_resource(ImageTarget::new(&device, UVec2::ONE, format));
    commands.insert_resource(Debugger {
        font_view,
        layout,
        pipeline: None,
    });
}

type ResizeView<'w, 's> = Query<'w, 's, &'static ExtractedCamera>;

/// Keep the offscreen image target at viewport size while the debugger is
/// on, and shrink it to 1×1 while it is off. Runs before the main
/// fullscreen pass so both see the same size within a frame.
pub fn resize_image_target(
    views: ResizeView<'_, '_>,
    on: Res<DebuggerOn>,
    device: Res<RenderDevice>,
    mut target: ResMut<ImageTarget>,
) {
    let size = if on.0 {
        let Some(size) = views
            .iter()
            .find_map(|camera| camera.physical_viewport_size)
        else {
            return;
        };
        size
    } else {
        UVec2::ONE
    };
    target.resize(size, &device);
}

type BlitView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ExtractedView,
        &'static ViewTarget,
        &'static DynamicUniformIndex<ShaderToyUniform>,
        // Only views the fullscreen-material plugin drives get a blit pass.
        &'static FullscreenMaterialPipelineId<ShaderToyUniform>,
    ),
>;

/// Resources [`debugger_blit_pass`] reads, bundled into one parameter.
#[derive(SystemParam)]
pub struct BlitParams<'w> {
    on: Res<'w, DebuggerOn>,
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: Res<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    shader: Res<'w, DebuggerShader>,
    debugger: ResMut<'w, Debugger>,
    image: Res<'w, ImageTarget>,
    render_device: Res<'w, RenderDevice>,
    data_uniforms: Res<'w, ComponentUniforms<ShaderToyUniform>>,
}

/// Draw the main-pass image with the debugger overlay on top to the view.
/// Runs after the fullscreen material pass, recording into the same command
/// encoder so the passes land in order. Does nothing while the debugger is
/// off (the main pass then renders to the view directly).
pub fn debugger_blit_pass(view: BlitView<'_, '_>, mut p: BlitParams, mut ctx: RenderContext) {
    if !p.on.0 {
        return;
    }
    let (extracted, view_target, settings_index, _marker) = view.into_inner();
    let target_format = extracted.target_format;

    if p.debugger.pipeline.map(|(f, _)| f) != Some(target_format) {
        let descriptor = RenderPipelineDescriptor {
            label: Some("debugger_blit_pipeline".into()),
            layout: vec![p.debugger.layout.clone()],
            vertex: p.fullscreen.vertex.clone(),
            fragment: Some(FragmentState {
                shader: p.shader.0.clone(),
                entry_point: Some(BLIT_ENTRY.into()),
                targets: vec![Some(ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            ..Default::default()
        };
        p.debugger.pipeline = Some((
            target_format,
            p.pipeline_cache.queue_render_pipeline(descriptor),
        ));
    }
    let Some(Some(pipeline)) = p
        .debugger
        .pipeline
        .map(|(_, id)| p.pipeline_cache.get_render_pipeline(id))
    else {
        return; // still compiling; the destination keeps its clear colour
    };
    let Some(settings_binding) = p.data_uniforms.uniforms().binding() else {
        return;
    };

    let bind_group = p.render_device.create_bind_group(
        "debugger_blit_bind_group",
        &p.pipeline_cache.get_bind_group_layout(&p.debugger.layout),
        &BindGroupEntries::sequential((&p.image.view, &p.debugger.font_view, settings_binding)),
    );

    let post_process = view_target.post_process_write();
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("debugger_blit_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post_process.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[settings_index.index()]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugger_wgsl_parses() {
        naga::front::wgsl::parse_str(DEBUGGER_WGSL)
            .expect("debugger module must parse with the shipped naga");
    }

    /// The blit shader embeds the shared uniform block; check it arrives
    /// intact (the per-member check lives in `compile.rs`).
    #[test]
    fn debugger_uniform_matches() {
        use bevy::render::render_resource::ShaderType;

        let module = naga::front::wgsl::parse_str(DEBUGGER_WGSL).unwrap();
        let handle = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ShaderToyUniforms"))
            .map(|(h, _)| h)
            .expect("ShaderToyUniforms present");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        assert_eq!(
            u64::from(layouter[handle].size),
            ShaderToyUniform::min_size().get()
        );
    }

    #[test]
    fn font_atlas_has_glyphs() {
        let atlas = font_atlas_pixels();
        assert_eq!(atlas.len(), (ATLAS_SIZE * ATLAS_SIZE) as usize);

        let cell_nonzero = |ch: u32| -> bool {
            let (col, row) = (ch % 16, ch / 16);
            (0..CELL).any(|y| {
                (0..CELL)
                    .any(|x| atlas[((row * CELL + y) * ATLAS_SIZE + col * CELL + x) as usize] > 0)
            })
        };
        let cell_zero = |ch: u32| -> bool { !cell_nonzero(ch) };

        assert!(cell_nonzero(u32::from(b'A')), "'A' cell must have ink");
        assert!(cell_nonzero(u32::from(b'0')), "'0' cell must have ink");
        assert!(
            cell_nonzero(28),
            "cell 28 (multiplication sign) must have ink"
        );
        assert!(cell_zero(32), "the space cell must stay blank");
        assert!(cell_zero(0), "control characters must stay blank");
    }
}
