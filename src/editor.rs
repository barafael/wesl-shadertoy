//! A code editor for egui with syntax highlighting.
//!
//! egui has no built-in code editor, so this builds one from `TextEdit`:
//!
//! * a custom `layouter` highlights the buffer into a [`LayoutJob`] and lays it
//!   out into a galley, so the `TextEdit` paints the colored text *itself* —
//!   the cursor and selection rectangles are computed from the very same galley
//!   whose glyphs are on screen and can never drift off them;
//! * the highlighted job is cached in [`HighlightCache`] and only rebuilt when
//!   the text or theme actually changed — egui re-runs the layouter every
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
use bevy_egui::egui::widgets::text_edit::TextEditState;
use bevy_egui::egui::{
    Align, Color32, Context, FontId, Frame, Galley, Id, Key, Label, Layout, Margin, Modifiers,
    ScrollArea, Sense, TextBuffer, TextEdit, TextWrapMode, Ui, Vec2, pos2,
};

use crate::highlight::highlight_job;
use crate::theme::Theme;

/// Cached highlight results, so the tokenizer runs only when the text or the
/// theme actually changed — not on every egui frame (egui calls the editor's
/// layouter once per frame, more while interacting).
#[derive(Resource, Default)]
pub struct HighlightCache {
    editor: Option<CachedJob<String>>,
    editor_gutter: Option<CachedJob<usize>>,
    viewer: Option<CachedJob<String>>,
    viewer_gutter: Option<CachedJob<usize>>,
}

/// A highlighted [`LayoutJob`], keyed by what it was built from (the source
/// text, or the line count for a gutter) plus the theme.
///
/// Only the job is cached, never the laid-out [`Galley`]: a galley's glyphs
/// are UV rects into egui's font atlas, and egui discards and rebuilds that
/// atlas when the fonts or text options change (installing the icon font on
/// frame one, switching theme) or when it fills up, and rasterizes anew when
/// the scale changes (zoom, DPI). egui's galley cache is flushed on all of
/// those; a galley kept across them paints stale texels — garbled or missing
/// text until the next edit. Laying the cached job out every frame is a hash
/// lookup in that cache.
struct CachedJob<K> {
    key: K,
    theme: Theme,
    job: LayoutJob,
}

/// The job cached in `slot` if it was built from `key` and `theme`, else a
/// fresh one from `build` (which then replaces the cached one).
fn cached_job<Q>(
    slot: &mut Option<CachedJob<Q::Owned>>,
    key: &Q,
    theme: Theme,
    build: impl FnOnce() -> LayoutJob,
) -> LayoutJob
where
    Q: PartialEq + ToOwned + ?Sized,
    Q::Owned: Borrow<Q>,
{
    if let Some(cached) = slot
        .as_ref()
        .filter(|c| c.theme == theme && c.key.borrow() == key)
    {
        return cached.job.clone();
    }
    let job = build();
    *slot = Some(CachedJob {
        key: key.to_owned(),
        theme,
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

pub struct EditorOutput {
    /// The text was edited this frame.
    pub changed: bool,
}

/// Show the highlighted code editor inside `ui`, colored with `theme`'s
/// palette, reusing (and updating) the highlighted jobs held in `cache`.
pub fn show_code_editor(
    ui: &mut Ui,
    text: &mut String,
    theme: Theme,
    cache: &mut HighlightCache,
) -> EditorOutput {
    let pal = theme.syntax();
    let ctx = ui.ctx().clone();
    let font_id = FontId::monospace(13.0);
    let default_color = ui.visuals().text_color();
    let margin = Margin {
        left: 4,
        right: 4,
        top: 2,
        bottom: 2,
    };

    // --- Tab / Shift-Tab handling (must run before TextEdit sees the key) ---
    let prev_id: Option<Id> = ctx.data(|d| d.get_temp(editor_id_marker()));
    let focused = prev_id.is_some_and(|id| ctx.memory(|m| m.focused() == Some(id)));
    if focused {
        let tab = ctx.input(|i| i.key_pressed(Key::Tab));
        let shift = ctx.input(|i| i.modifiers.shift);
        if tab {
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
    let gutter_width = (display_lines.ilog10() as usize + 1).max(2) as f32
        * ctx.fonts_mut(|f| f.glyph_width(&font_id, '0'))
        + 12.0;

    let mut changed = false;
    ScrollArea::both()
        .id_salt("code_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                // --- line-number gutter (job rebuilt only when the line
                // count or theme changes, not every frame) ---
                let weak = ui.visuals().weak_text_color();
                let job = cached_job(&mut cache.editor_gutter, &display_lines, theme, || {
                    gutter_job(&font_id, weak, display_lines)
                });
                let gutter_galley = ui.fonts_mut(|f| f.layout_job(job));
                let (gutter_rect, _) = ui.allocate_at_least(
                    Vec2::new(gutter_width, gutter_galley.size().y),
                    Sense::hover(),
                );
                ui.painter().galley(
                    pos2(
                        gutter_rect.max.x - gutter_galley.size().x,
                        gutter_rect.min.y + margin.top as f32,
                    ),
                    gutter_galley,
                    ui.visuals().weak_text_color(),
                );

                // --- the editor: TextEdit paints the highlighted job itself,
                // so cursor and selection always sit on the visible glyphs ---
                let mut layouter =
                    |ui: &Ui, buf: &dyn TextBuffer, _wrap_width: f32| -> Arc<Galley> {
                        let src = buf.as_str();
                        let job = cached_job(&mut cache.editor, src, theme, || {
                            let mut job = highlight_job(src, font_id.clone(), default_color, &pal);
                            job.keep_trailing_whitespace = true;
                            job
                        });
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                let response = ui.add(
                    TextEdit::multiline(text)
                        .font(font_id.clone())
                        .margin(margin)
                        .desired_width(f32::INFINITY)
                        .frame(Frame::NONE)
                        .layouter(&mut layouter),
                );
                changed = response.changed();

                ctx.data_mut(|d| d.insert_temp(editor_id_marker(), response.id));
            });
        });

    EditorOutput { changed }
}

/// Show a read-only, syntax-highlighted code view with a line-number gutter.
///
/// This is the generated-WGSL viewer: same tokenizer and colors as the
/// editor, but the text is painted as a selectable [`Label`] instead of a
/// `TextEdit`, since there is nothing to edit. `TextWrapMode::Extend` is what
/// keeps long composed-WGSL lines on one row (a plain Label would wrap them
/// at the window width and break the gutter alignment).
pub fn show_code_view(ui: &mut Ui, text: &str, theme: Theme, cache: &mut HighlightCache) {
    let pal = theme.syntax();
    let ctx = ui.ctx().clone();
    let font_id = FontId::monospace(13.0);
    let default_color = ui.visuals().text_color();

    let display_lines = text.matches('\n').count() + 1;
    let gutter_width = (display_lines.ilog10() as usize + 1).max(2) as f32
        * ctx.fonts_mut(|f| f.glyph_width(&font_id, '0'))
        + 12.0;

    ScrollArea::both()
        .id_salt("code_view_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Top-aligned so gutter rows and code rows share the same baselines.
            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                // --- line-number gutter (cached job) ---
                let weak = ui.visuals().weak_text_color();
                let job = cached_job(&mut cache.viewer_gutter, &display_lines, theme, || {
                    gutter_job(&font_id, weak, display_lines)
                });
                let gutter_galley = ui.fonts_mut(|f| f.layout_job(job));
                let (gutter_rect, _) = ui.allocate_at_least(
                    Vec2::new(gutter_width, gutter_galley.size().y),
                    Sense::hover(),
                );
                ui.painter().galley(
                    pos2(
                        gutter_rect.max.x - gutter_galley.size().x,
                        gutter_rect.min.y,
                    ),
                    gutter_galley,
                    ui.visuals().weak_text_color(),
                );

                // --- highlighted code, selectable for copy-paste ---
                let job = cached_job(&mut cache.viewer, text, theme, || {
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
    use bevy_egui::egui::epaint::{ClippedShape, ImageData, Shape};
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
        /// Dark and light visuals differ in `TextOptions`, which also
        /// rebuilds the atlas on the next pass.
        ThemeSwitch,
        /// Zoom / DPI change: glyphs are rasterized anew at the new scale.
        Zoom,
    }

    /// Show the pane for a few unchanged-text frames, trigger `rebuild` on
    /// frame 2, and check every frame's painted glyphs — each trigger in a
    /// fresh context so one failing cannot mask another. The frame mirrors
    /// the app's: `theme::apply` runs first, then the pane.
    fn survives_atlas_rebuilds(mut show: impl FnMut(&mut Ui, Theme, &mut HighlightCache)) {
        for rebuild in [Rebuild::IconFont, Rebuild::ThemeSwitch, Rebuild::Zoom] {
            let ctx = Context::default();
            let mut cache = HighlightCache::default();
            let mut gpu = GpuFontTexture::default();
            for frame in 0..4 {
                let trigger = |r| frame == 2 && rebuild == r;
                let theme = if frame >= 2 && rebuild == Rebuild::ThemeSwitch {
                    Theme::Light
                } else {
                    Theme::Dark
                };
                if trigger(Rebuild::Zoom) {
                    ctx.set_zoom_factor(1.5);
                }
                let mut output = ctx.run_ui(RawInput::default(), |ui| {
                    if trigger(Rebuild::IconFont) {
                        ui.ctx().set_fonts(crate::lucide_fonts());
                    }
                    crate::theme::apply(ui.ctx(), theme);
                    show(ui, theme, &mut cache);
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
        let mut text = crate::presets::ALL[0].code.to_string();
        survives_atlas_rebuilds(|ui, theme, cache| {
            show_code_editor(ui, &mut text, theme, cache);
        });
    }

    #[test]
    fn viewer_text_survives_font_atlas_rebuilds() {
        let text = crate::presets::ALL[0].code;
        survives_atlas_rebuilds(|ui, theme, cache| show_code_view(ui, text, theme, cache));
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
        let mut text = crate::presets::ALL[0].code.to_string();
        for _ in 0..3 {
            ctx.run_ui(bevy_egui::egui::RawInput::default(), |ui| {
                show_code_editor(ui, &mut text, Theme::Dark, &mut cache);
            })
            .drop_without_applying_deltas();
        }
        let cached = cache.editor.as_ref().expect("editor job cached");
        assert_eq!(cached.job.text, text);
        assert!(rows(&ctx, &cached.job) > 10);
        let gutter = cache.editor_gutter.as_ref().expect("editor gutter cached");
        assert_eq!(rows(&ctx, &gutter.job), text.matches('\n').count() + 1);
    }

    #[test]
    fn viewer_cache_holds_full_job() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let text = crate::presets::ALL[0].code;
        for _ in 0..3 {
            ctx.run_ui(bevy_egui::egui::RawInput::default(), |ui| {
                show_code_view(ui, text, Theme::Dark, &mut cache);
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
