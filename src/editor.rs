//! A code editor for egui with syntax highlighting.
//!
//! egui has no built-in code editor, so this builds one from `TextEdit`:
//!
//! * a custom `layouter` highlights the buffer into a [`LayoutJob`] and lays it
//!   out into a galley, so the `TextEdit` paints the colored text *itself* —
//!   the cursor and selection rectangles are computed from the very same galley
//!   whose glyphs are on screen and can never drift off them;
//! * the highlighted job is cached in [`HighlightCache`] and only rebuilt when
//!   the text actually changed — egui re-runs the layouter every
//!   frame, and re-tokenizing the whole document each time is pure waste. The
//!   job is laid out every frame through egui's own galley cache, which is a
//!   hash lookup (see [`CachedJob`] for why the galley itself must not be kept);
//! * wrapping is disabled in the job: the editor scrolls horizontally instead,
//!   which keeps the painted line-number gutter aligned with the code rows.
//!
//! The read-only WGSL viewer uses the same tokenizer, painted as a selectable
//! [`Label`] with [`TextWrapMode::Extend`] for the same reason (its job is
//! cached the same way).

use std::borrow::Borrow;
use std::sync::Arc;

use bevy::prelude::Resource;
use bevy_egui::egui::text::CCursor;
use bevy_egui::egui::text::LayoutJob;
use bevy_egui::egui::text_selection::CCursorRange;
use bevy_egui::egui::widgets::text_edit::{TextEditOutput, TextEditState};
use bevy_egui::egui::{
    Align, Color32, Context, Event, FontId, Frame, Galley, Id, Key, Label, Layout, Margin,
    Modifiers, Painter, Rect, ScrollArea, Sense, Shape, Stroke, TextBuffer, TextEdit, TextWrapMode,
    Ui, Vec2, Visuals, pos2,
};

use crate::analyzer::{Diagnostic, Severity};
use crate::highlight::highlight_job;
use crate::theme::SYNTAX;

/// Cached highlight results, so the tokenizer runs only when the text
/// actually changed — not on every egui frame (egui calls the editor's
/// layouter once per frame, more while interacting).
#[derive(Resource, Default)]
pub struct HighlightCache {
    editor: Option<CachedJob<String>>,
    editor_gutter: Option<CachedJob<usize>>,
    viewer: Option<CachedJob<String>>,
    viewer_gutter: Option<CachedJob<usize>>,
}

/// A highlighted [`LayoutJob`], keyed by what it was built from (the source
/// text, or the line count for a gutter).
///
/// Only the job is cached, never the laid-out [`Galley`]: a galley's glyphs
/// are UV rects into egui's font atlas, and egui discards and rebuilds that
/// atlas when the fonts or text options change (installing the icon font on
/// frame one, the theme's text options) or when it fills up, and rasterizes anew when
/// the scale changes (zoom, DPI). egui's galley cache is flushed on all of
/// those; a galley kept across them paints stale texels — garbled or missing
/// text until the next edit. Laying the cached job out every frame is a hash
/// lookup in that cache.
struct CachedJob<K> {
    key: K,
    job: LayoutJob,
}

/// The job cached in `slot` if it was built from `key`, else a
/// fresh one from `build` (which then replaces the cached one).
fn cached_job<Q>(
    slot: &mut Option<CachedJob<Q::Owned>>,
    key: &Q,
    build: impl FnOnce() -> LayoutJob,
) -> LayoutJob
where
    Q: PartialEq + ToOwned + ?Sized,
    Q::Owned: Borrow<Q>,
{
    if let Some(cached) = slot.as_ref().filter(|c| c.key.borrow() == key) {
        return cached.job.clone();
    }
    let job = build();
    *slot = Some(CachedJob {
        key: key.to_owned(),
        job: job.clone(),
    });
    job
}

/// The line-number column, one number per row. Shared by both gutters so
/// they cannot drift apart (a `collect::<String>()` once stacked every number
/// on the first row of the viewer gutter).
fn gutter_job(font_id: &FontId, color: Color32, lines: usize) -> LayoutJob {
    let gutter_text: String = (1..=lines)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    LayoutJob::simple(gutter_text, font_id.clone(), color, f32::INFINITY)
}

/// Stable id used to remember which `TextEdit` id the editor got last frame
/// (so Tab handling can run *before* the widget is added).
fn editor_id_marker() -> Id {
    Id::new("wesl_shadertoy_editor_id")
}

const INDENT: &str = "    ";

/// Space between the right end of the line numbers and the code.
const GUTTER_GAP: f32 = 10.0;

/// Width of the line-number gutter for `lines` lines, including
/// [`GUTTER_GAP`] (the numbers sit right-aligned against the gap).
fn gutter_width(ctx: &Context, font_id: &FontId, lines: usize) -> f32 {
    let digits = (lines.ilog10() as usize + 1).max(2) as f32;
    digits * ctx.fonts_mut(|f| f.glyph_width(font_id, '0')) + 8.0 + GUTTER_GAP
}

/// Show the highlighted code editor inside `ui`, reusing (and updating) the highlighted jobs held in `cache`.
/// `diagnostics` (char ranges into `text`) are squiggled under the code.
pub fn show_code_editor(
    ui: &mut Ui,
    text: &mut String,
    cache: &mut HighlightCache,
    diagnostics: &[Diagnostic],
) {
    let pal = SYNTAX;
    let ctx = ui.ctx().clone();
    let font_id = FontId::monospace(13.0);
    let default_color = ui.visuals().text_color();
    let margin = Margin {
        left: 4,
        right: 4,
        top: 2,
        bottom: 2,
    };

    // --- Tab / Shift-Tab handling (must run before TextEdit sees the key;
    // the TextEdit's `lock_focus` keeps egui from moving focus on Tab) ---
    let prev_id: Option<Id> = ctx.data(|d| d.get_temp(editor_id_marker()));
    let focused = prev_id.is_some_and(|id| ctx.memory(|m| m.focused() == Some(id)));
    if focused {
        // Shift comes from the key event itself: the frame's modifier state
        // may already have moved on by the end of the input batch.
        let tab = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Key {
                    key: Key::Tab,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(modifiers.shift),
                _ => None,
            })
        });
        if let Some(shift) = tab {
            if let Some(id) = prev_id {
                handle_tab(&ctx, id, text, shift);
            }
            ctx.input_mut(|i| {
                i.consume_key(Modifiers::NONE, Key::Tab);
                i.consume_key(Modifiers::SHIFT, Key::Tab);
            });
        }
    }

    let display_lines = text.matches('\n').count() + 1;
    let gutter_width = gutter_width(&ctx, &font_id, display_lines);

    ScrollArea::both()
        .id_salt("code_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Top-aligned, so the gutter's first row starts level with the
            // TextEdit's (`horizontal` centers, and the TextEdit is taller by
            // its margin, which pushed the numbers below their lines).
            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                // --- line-number gutter (job rebuilt only when the line
                // count changes, not every frame) ---
                let weak = ui.visuals().weak_text_color();
                let job = cached_job(&mut cache.editor_gutter, &display_lines, || {
                    gutter_job(&font_id, weak, display_lines)
                });
                let gutter_galley = ui.fonts_mut(|f| f.layout_job(job));
                let (gutter_rect, _) = ui.allocate_at_least(
                    Vec2::new(gutter_width, gutter_galley.size().y),
                    Sense::hover(),
                );

                // --- the editor: TextEdit paints the highlighted job itself,
                // so cursor and selection always sit on the visible glyphs ---
                let mut layouter =
                    |ui: &Ui, buf: &dyn TextBuffer, _wrap_width: f32| -> Arc<Galley> {
                        let src = buf.as_str();
                        let job = cached_job(&mut cache.editor, src, || {
                            let mut job = highlight_job(src, font_id.clone(), default_color, &pal);
                            job.keep_trailing_whitespace = true;
                            job
                        });
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                let output = TextEdit::multiline(text)
                    .font(font_id.clone())
                    .margin(margin)
                    .desired_width(f32::INFINITY)
                    .frame(Frame::NONE)
                    // Without this, egui moves keyboard focus to the
                    // next widget on Tab before `handle_tab` runs.
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui);

                // Numbers on the rows where the TextEdit actually put its
                // text (it ignores the top margin, so `margin.top` would put
                // them a couple of pixels low). Its left margin is part of
                // the gap.
                let gap = GUTTER_GAP - margin.left as f32;
                ui.painter().galley(
                    pos2(
                        gutter_rect.max.x - gap - gutter_galley.size().x,
                        output.galley_pos.y,
                    ),
                    gutter_galley,
                    weak,
                );
                paint_diagnostics(ui, &output, gutter_rect, diagnostics);

                ctx.data_mut(|d| d.insert_temp(editor_id_marker(), output.response.response.id));
            });
        });
}

/// Squiggle each diagnostic under its text, tick its first line in the
/// gutter, and show the messages while the pointer rests on one.
///
/// Drawn from the galley the `TextEdit` just painted, so the marks sit on
/// the glyphs they refer to (including while scrolled).
fn paint_diagnostics(ui: &Ui, output: &TextEditOutput, gutter: Rect, diagnostics: &[Diagnostic]) {
    let galley = &output.galley;
    // Where TextEdit put the galley (it paints at this offset too).
    let origin = output.galley_pos - Vec2::new(galley.rect.left(), 0.0);
    let chars = galley.end().index.0;
    let painter = ui.painter();
    let mut spans: Vec<(Rect, &Diagnostic)> = Vec::new();
    for diagnostic in diagnostics {
        let color = severity_color(ui.visuals(), diagnostic.severity);
        let at = |i: usize| galley.layout_from_cursor(CCursor::new(i.min(chars)));
        let (start, end) = (at(diagnostic.range.start), at(diagnostic.range.end));
        for row in start.row..=end.row {
            let Some(placed) = galley.rows.get(row) else {
                break;
            };
            let left = if row == start.row {
                placed.row.x_offset(start.column)
            } else {
                0.0
            };
            let right = if row == end.row {
                placed.row.x_offset(end.column)
            } else {
                placed.row.size.x
            };
            let line = placed.rect().translate(origin.to_vec2());
            let x = origin.x + placed.pos.x;
            let span = Rect::from_x_y_ranges(x + left..=x + right.max(left + 4.0), line.y_range());
            squiggle(painter, span, color);
            spans.push((span, diagnostic));
            if row == start.row {
                let tick = Rect::from_x_y_ranges(gutter.min.x..=gutter.min.x + 3.0, line.y_range());
                painter.rect_filled(tick, 0.0, color);
            }
        }
    }

    let response = &output.response.response;
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    let mut hits: Vec<&Diagnostic> = spans
        .iter()
        .filter(|(span, _)| span.contains(pointer))
        .map(|(_, d)| *d)
        .collect();
    hits.dedup_by(|a, b| std::ptr::eq(*a, *b));
    if !hits.is_empty() {
        response.clone().on_hover_ui_at_pointer(|ui| {
            for d in hits {
                ui.colored_label(severity_color(ui.visuals(), d.severity), &d.message);
            }
        });
    }
}

fn severity_color(visuals: &Visuals, severity: Severity) -> Color32 {
    match severity {
        Severity::Error => visuals.error_fg_color,
        Severity::Warning => visuals.warn_fg_color,
        Severity::Info => visuals.hyperlink_color,
    }
}

/// A zigzag along the bottom of `span`.
fn squiggle(painter: &Painter, span: Rect, color: Color32) {
    const STEP: f32 = 2.0;
    const HEIGHT: f32 = 1.5;
    let y = span.bottom() - 1.0;
    let mut points = Vec::new();
    let mut x = span.left();
    let mut up = false;
    while x < span.right() {
        points.push(pos2(x, if up { y - HEIGHT } else { y }));
        x += STEP;
        up = !up;
    }
    points.push(pos2(span.right(), if up { y - HEIGHT } else { y }));
    painter.add(Shape::line(points, Stroke::new(1.0, color)));
}

/// Show a read-only, syntax-highlighted code view with a line-number gutter.
///
/// This is the generated-WGSL viewer: same tokenizer and colors as the
/// editor, but the text is painted as a selectable [`Label`] instead of a
/// `TextEdit`, since there is nothing to edit. `TextWrapMode::Extend` is what
/// keeps long composed-WGSL lines on one row (a plain Label would wrap them
/// at the window width and break the gutter alignment).
pub fn show_code_view(ui: &mut Ui, text: &str, cache: &mut HighlightCache) {
    let pal = SYNTAX;
    let ctx = ui.ctx().clone();
    let font_id = FontId::monospace(13.0);
    let default_color = ui.visuals().text_color();

    let display_lines = text.matches('\n').count() + 1;
    let gutter_width = gutter_width(&ctx, &font_id, display_lines);

    ScrollArea::both()
        .id_salt("code_view_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Top-aligned so gutter rows and code rows share the same baselines.
            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                // --- line-number gutter (cached job) ---
                let weak = ui.visuals().weak_text_color();
                let job = cached_job(&mut cache.viewer_gutter, &display_lines, || {
                    gutter_job(&font_id, weak, display_lines)
                });
                let gutter_galley = ui.fonts_mut(|f| f.layout_job(job));
                let (gutter_rect, _) = ui.allocate_at_least(
                    Vec2::new(gutter_width, gutter_galley.size().y),
                    Sense::hover(),
                );
                ui.painter().galley(
                    pos2(
                        gutter_rect.max.x - GUTTER_GAP - gutter_galley.size().x,
                        gutter_rect.min.y,
                    ),
                    gutter_galley,
                    ui.visuals().weak_text_color(),
                );

                // --- highlighted code, selectable for copy-paste ---
                let job = cached_job(&mut cache.viewer, text, || {
                    highlight_job(text, font_id.clone(), default_color, &pal)
                });
                ui.add(
                    Label::new(job)
                        .selectable(true)
                        .wrap_mode(TextWrapMode::Extend),
                );
            });
        });
}

/// Indent (or with `shift`, dedent) at the cursor.
fn handle_tab(ctx: &Context, id: Id, text: &mut String, shift: bool) {
    let Some(mut state) = TextEditState::load(ctx, id) else {
        return;
    };
    let Some(range) = state.cursor.char_range() else {
        return;
    };
    let ccursor = range.primary;
    let byte = byte_index_from_char_index(text, ccursor.index.0);

    if shift {
        let line_start = text[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let removed = text[line_start..byte]
            .chars()
            .take_while(|c| *c == ' ')
            .count()
            .min(INDENT.len());
        if removed > 0 {
            text.replace_range(line_start..line_start + removed, "");
        }
        let new_byte = byte.saturating_sub(removed);
        let new_cc = char_index_from_byte_index(text, new_byte);
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(new_cc))));
    } else {
        text.insert_str(byte, INDENT);
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(
                ccursor.index + INDENT.len(),
            ))));
    }
    state.store(ctx, id);
}

/// egui keeps these functions private to `text_cursor_cache`; ours are tiny.
fn byte_index_from_char_index(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn char_index_from_byte_index(s: &str, byte_index: usize) -> usize {
    let mut b = byte_index.min(s.len());
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::epaint::{ClippedShape, ColorMode, ImageData};
    use bevy_egui::egui::{ColorImage, FullOutput, RawInput, TextureId};

    /// Stand-in for the renderer's copy of egui's font texture: applies each
    /// frame's `textures_delta` exactly like a GPU backend uploads it.
    #[derive(Default)]
    struct GpuFontTexture(Option<ColorImage>);

    impl GpuFontTexture {
        fn upload(&mut self, output: &FullOutput) {
            let Some(deltas) = output.textures_delta.set.get(&TextureId::default()) else {
                return;
            };
            for delta in deltas {
                let ImageData::Color(patch) = &delta.image;
                match delta.pos {
                    None => self.0 = Some((**patch).clone()),
                    Some([x0, y0]) => {
                        let tex = self.0.as_mut().expect("partial upload before a full one");
                        let w = tex.width();
                        for y in 0..patch.height() {
                            for x in 0..patch.width() {
                                tex.pixels[(y0 + y) * w + x0 + x] =
                                    patch.pixels[y * patch.width() + x];
                            }
                        }
                    }
                }
            }
        }
    }

    fn painted_galleys(shapes: &[ClippedShape]) -> Vec<Arc<Galley>> {
        fn walk(shape: &Shape, out: &mut Vec<Arc<Galley>>) {
            match shape {
                Shape::Text(t) => out.push(t.galley.clone()),
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    fn texels(img: &ColorImage, min: [u16; 2], max: [u16; 2]) -> Option<Vec<Color32>> {
        let (x1, y1) = (max[0] as usize, max[1] as usize);
        if x1 > img.width() || y1 > img.height() {
            return None;
        }
        let mut px = Vec::new();
        for y in min[1] as usize..y1 {
            for x in min[0] as usize..x1 {
                px.push(img.pixels[y * img.width() + x]);
            }
        }
        Some(px)
    }

    /// The on-screen symptom, checked directly: every glyph a painted galley
    /// references must be in the uploaded font texture, pixel-identical to
    /// the glyph a fresh layout of the same text gets from the live atlas.
    fn assert_glyphs_uploaded(
        ctx: &Context,
        gpu: &GpuFontTexture,
        output: &FullOutput,
        what: &str,
    ) {
        let gpu = gpu.0.as_ref().expect("font texture uploaded");
        let galleys = painted_galleys(&output.shapes);
        assert!(
            galleys.iter().any(|g| g.rows.len() > 10),
            "{what}: the code pane was not painted"
        );
        for painted in galleys {
            let fresh = ctx.fonts_mut(|f| f.layout_job((*painted.job).clone()));
            let cpu = ctx.fonts(|f| f.image());
            let pairs = painted
                .rows
                .iter()
                .zip(&fresh.rows)
                .flat_map(|(a, b)| a.row.glyphs.iter().zip(&b.row.glyphs));
            for (p, f) in pairs {
                if f.uv_rect.is_nothing() {
                    continue; // whitespace
                }
                let shown = texels(gpu, p.uv_rect.min, p.uv_rect.max);
                let truth = texels(&cpu, f.uv_rect.min, f.uv_rect.max);
                assert!(
                    shown == truth,
                    "{what}: glyph {:?} painted from font-texture texels {:?}..{:?}, \
                     which do not hold it (the live atlas has it at {:?}..{:?})",
                    p.chr,
                    p.uv_rect.min,
                    p.uv_rect.max,
                    f.uv_rect.min,
                    f.uv_rect.max,
                );
            }
        }
    }

    /// What invalidates the glyphs of an already laid-out galley.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Rebuild {
        /// `install_fonts` on the app's first frame: `set_fonts` makes egui
        /// build a new atlas at the start of the next pass.
        IconFont,
        /// Zoom / DPI change: glyphs are rasterized anew at the new scale.
        Zoom,
    }

    /// Show the pane for a few unchanged-text frames, trigger `rebuild` on
    /// frame 2, and check every frame's painted glyphs — each trigger in a
    /// fresh context so one failing cannot mask another. The frame mirrors
    /// the app's: `theme::apply` runs first, then the pane.
    fn survives_atlas_rebuilds(mut show: impl FnMut(&mut Ui, &mut HighlightCache)) {
        for rebuild in [Rebuild::IconFont, Rebuild::Zoom] {
            let ctx = Context::default();
            let mut cache = HighlightCache::default();
            let mut gpu = GpuFontTexture::default();
            for frame in 0..4 {
                let trigger = |r| frame == 2 && rebuild == r;
                if trigger(Rebuild::Zoom) {
                    ctx.set_zoom_factor(1.5);
                }
                let mut output = ctx.run_ui(RawInput::default(), |ui| {
                    if trigger(Rebuild::IconFont) {
                        ui.ctx().set_fonts(crate::lucide_fonts());
                    }
                    crate::theme::apply(ui.ctx());
                    show(ui, &mut cache);
                });
                gpu.upload(&output);
                // Applied above; clearing first keeps a failed assert from
                // aborting in `TexturesDelta`'s unapplied-deltas drop check.
                output.textures_delta.clear();
                let what = format!("{rebuild:?}, frame {frame}");
                assert_glyphs_uploaded(&ctx, &gpu, &output, &what);
            }
        }
    }

    #[test]
    fn editor_text_survives_font_atlas_rebuilds() {
        let mut text = crate::examples::default().code.to_string();
        survives_atlas_rebuilds(|ui, cache| {
            show_code_editor(ui, &mut text, cache, &[]);
        });
    }

    #[test]
    fn viewer_text_survives_font_atlas_rebuilds() {
        let text = crate::examples::default().code;
        survives_atlas_rebuilds(|ui, cache| show_code_view(ui, text, cache));
    }

    /// Tab indents at the cursor and keeps focus in the editor (egui moves
    /// focus on Tab unless the TextEdit locks it), and Shift-Tab dedents.
    #[test]
    fn tab_indents_and_keeps_focus() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let mut text = String::from("fn f() {}\n");
        // What a backend sends for one (Shift-)Tab stroke within a frame:
        // modifier down, key press + release, modifier up.
        let tab = |shift: bool| {
            let modifiers = if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            };
            let key = |pressed| Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            RawInput {
                events: vec![
                    Event::ModifiersChanged(modifiers),
                    key(true),
                    key(false),
                    Event::ModifiersChanged(Modifiers::NONE),
                ],
                ..Default::default()
            }
        };
        let mut frame = |input: RawInput, text: &mut String| {
            ctx.run_ui(input, |ui| {
                show_code_editor(ui, text, &mut cache, &[]);
            })
            .drop_without_applying_deltas();
        };

        frame(RawInput::default(), &mut text);
        let id: Id = ctx
            .data(|d| d.get_temp(editor_id_marker()))
            .expect("editor id recorded");
        ctx.memory_mut(|m| m.request_focus(id));
        let mut state = TextEditState::load(&ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
        state.store(&ctx, id);
        // The TextEdit registers its focus-lock filter while it is drawn
        // focused (in the app: the frame of the click that focused it).
        frame(RawInput::default(), &mut text);

        for (shift, expected) in [(false, 4), (false, 8), (true, 4)] {
            frame(tab(shift), &mut text);
            assert_eq!(text, format!("{}fn f() {{}}\n", " ".repeat(expected)));
            assert_eq!(ctx.memory(|m| m.focused()), Some(id), "tab moved focus");
        }
    }

    /// Lay out a cached job the way the widgets do.
    fn rows(ctx: &Context, job: &LayoutJob) -> usize {
        ctx.fonts_mut(|f| f.layout_job(job.clone())).rows.len()
    }

    /// Headless check that the cached editor job really contains the full
    /// source after several frames (i.e. the cache never serves a stale or
    /// partial job).
    #[test]
    fn editor_cache_holds_full_job() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let mut text = crate::examples::default().code.to_string();
        for _ in 0..3 {
            ctx.run_ui(bevy_egui::egui::RawInput::default(), |ui| {
                show_code_editor(ui, &mut text, &mut cache, &[]);
            })
            .drop_without_applying_deltas();
        }
        let cached = cache.editor.as_ref().expect("editor job cached");
        assert_eq!(cached.job.text, text);
        assert!(rows(&ctx, &cached.job) > 10);
        let gutter = cache.editor_gutter.as_ref().expect("editor gutter cached");
        assert_eq!(rows(&ctx, &gutter.job), text.matches('\n').count() + 1);
    }

    /// A diagnostic is squiggled exactly under its characters and ticked in
    /// the gutter, in the theme's error color.
    #[test]
    fn diagnostics_are_squiggled_under_their_text() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let mut text = String::from("fn f() {\n    let t: u32 = 1.0;\n}\n");
        let flagged = 9 + 17..9 + 20; // `1.0` on the second line
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[flagged.clone()].iter().collect::<String>(), "1.0");
        let diagnostics = [Diagnostic {
            range: flagged,
            severity: Severity::Error,
            message: "expected u32, found f32".into(),
        }];

        let mut frame = || {
            let mut out = ctx.run_ui(RawInput::default(), |ui| {
                crate::theme::apply(ui.ctx());
                show_code_editor(ui, &mut text, &mut cache, &diagnostics);
            });
            out.textures_delta.clear();
            out
        };
        frame();
        let output = frame();
        let red = crate::theme::visuals().error_fg_color;

        let mut paths = Vec::new();
        let mut ticks = Vec::new();
        for clipped in &output.shapes {
            match &clipped.shape {
                Shape::Path(p) if p.stroke.color == ColorMode::Solid(red) => {
                    paths.push(p.points.clone())
                }
                Shape::Rect(r) if r.fill == red => ticks.push(r.rect),
                _ => {}
            }
        }
        assert_eq!(paths.len(), 1, "one squiggle");
        assert_eq!(ticks.len(), 1, "one gutter tick");

        // Same font as the editor: the squiggle spans three glyph advances,
        // starting after `    let t: u32 = ` (17 columns of code).
        let advance = ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(13.0), '0'));
        let xs = paths[0].iter().map(|p| p.x);
        let (left, right) = xs.fold((f32::MAX, f32::MIN), |(l, r), x| (l.min(x), r.max(x)));
        assert!(
            (right - left - 3.0 * advance).abs() < 1.0,
            "width {}",
            right - left
        );
        let code_left = ticks[0].min.x; // gutter's left edge
        assert!(
            left > code_left + 17.0 * advance,
            "squiggle starts at {left}"
        );
    }

    #[test]
    fn viewer_cache_holds_full_job() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let text = crate::examples::default().code;
        for _ in 0..3 {
            ctx.run_ui(bevy_egui::egui::RawInput::default(), |ui| {
                show_code_view(ui, text, &mut cache);
            })
            .drop_without_applying_deltas();
        }
        let cached = cache.viewer.as_ref().expect("viewer job cached");
        assert_eq!(cached.job.text, text);
        let gutter = cache.viewer_gutter.as_ref().expect("viewer gutter cached");
        // One row per line — the original bug stacked every number on row 1.
        assert_eq!(rows(&ctx, &gutter.job), text.matches('\n').count() + 1);
        assert!(gutter.job.text.starts_with("1\n2\n3\n"));
    }
}
