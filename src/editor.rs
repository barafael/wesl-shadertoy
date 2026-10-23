//! A code editor for egui with syntax highlighting.
//!
//! egui has no built-in code editor, so this builds one from `TextEdit`:
//!
//! * a custom `layouter` highlights the buffer into a [`LayoutJob`] and lays it
//!   out into a galley, so the `TextEdit` paints the colored text *itself* —
//!   the cursor and selection rectangles are computed from the very same galley
//!   whose glyphs are on screen and can never drift off them;
//! * the galley is cached in [`HighlightCache`] and only rebuilt when the text
//!   or theme actually changed — egui re-runs the layouter every frame, and
//!   re-tokenizing the whole document each time is pure waste;
//! * wrapping is disabled in the job: the editor scrolls horizontally instead,
//!   which keeps the painted line-number gutter aligned with the code rows.
//!
//! The read-only WGSL viewer uses the same tokenizer, painted as a selectable
//! [`Label`] with [`TextWrapMode::Extend`] for the same reason (its job is
//! cached the same way).

use std::sync::Arc;

use bevy::prelude::Resource;
use bevy_egui::egui::text::CCursor;
use bevy_egui::egui::text::LayoutJob;
use bevy_egui::egui::text_selection::CCursorRange;
use bevy_egui::egui::widgets::text_edit::TextEditState;
use bevy_egui::egui::{
    Align, Context, FontId, Frame, Galley, Id, Key, Label, Layout, Margin, Modifiers, ScrollArea,
    Sense, TextBuffer, TextEdit, TextWrapMode, Ui, Vec2, pos2,
};

use crate::highlight::highlight_job;
use crate::theme::Theme;

/// Cached highlight results, so the tokenizer runs only when the text or the
/// theme actually changed — not on every egui frame (egui calls the editor's
/// layouter once per frame, more while interacting).
#[derive(Resource, Default)]
pub struct HighlightCache {
    editor: Option<CachedGalley>,
    editor_gutter: Option<CachedGutter>,
    viewer: Option<CachedJob>,
    viewer_gutter: Option<CachedGutter>,
}

/// The editor's laid-out code: keyed by the exact text + theme.
struct CachedGalley {
    text: String,
    theme: Theme,
    galley: Arc<Galley>,
}

/// The viewer's highlighted job (a `Label` needs an owned job each frame;
/// cloning a cached one is far cheaper than re-tokenizing).
struct CachedJob {
    text: String,
    theme: Theme,
    job: LayoutJob,
}

/// A painted line-number column: keyed by line count + theme.
struct CachedGutter {
    lines: usize,
    theme: Theme,
    galley: Arc<Galley>,
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
/// palette, reusing (and updating) the laid-out galley held in `cache`.
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

                // --- line-number gutter (cached: rebuilt only when the line
                // count or theme changes, not every frame) ---
                let gutter_galley = match cache
                    .editor_gutter
                    .as_ref()
                    .filter(|g| g.lines == display_lines && g.theme == theme)
                    .map(|g| g.galley.clone())
                {
                    Some(galley) => galley,
                    None => {
                        let gutter_text: String = (1..=display_lines)
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let mut gutter_job = LayoutJob::simple(
                            gutter_text,
                            font_id.clone(),
                            ui.visuals().weak_text_color(),
                            f32::INFINITY,
                        );
                        gutter_job.wrap.max_width = f32::INFINITY;
                        let galley = ctx.fonts_mut(|f| f.layout_job(gutter_job));
                        cache.editor_gutter = Some(CachedGutter {
                            lines: display_lines,
                            theme,
                            galley: galley.clone(),
                        });
                        galley
                    }
                };
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
                        let hit = cache
                            .editor
                            .as_ref()
                            .filter(|c| c.theme == theme && c.text == src)
                            .map(|c| c.galley.clone());
                        if let Some(galley) = hit {
                            return galley;
                        }
                        let mut job = highlight_job(src, font_id.clone(), default_color, &pal);
                        // No wrapping: horizontal scroll keeps gutter rows aligned.
                        job.wrap.max_width = f32::INFINITY;
                        job.keep_trailing_whitespace = true;
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        cache.editor = Some(CachedGalley {
                            text: src.to_string(),
                            theme,
                            galley: galley.clone(),
                        });
                        galley
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
                // --- line-number gutter (cached painted galley) ---
                let gutter_galley = match cache
                    .viewer_gutter
                    .as_ref()
                    .filter(|g| g.lines == display_lines && g.theme == theme)
                    .map(|g| g.galley.clone())
                {
                    Some(galley) => galley,
                    None => {
                        let gutter_text: String =
                            (1..=display_lines).map(|i| i.to_string()).collect();
                        let mut gutter_job = LayoutJob::simple(
                            gutter_text,
                            font_id.clone(),
                            ui.visuals().weak_text_color(),
                            f32::INFINITY,
                        );
                        gutter_job.wrap.max_width = f32::INFINITY;
                        let galley = ctx.fonts_mut(|f| f.layout_job(gutter_job));
                        cache.viewer_gutter = Some(CachedGutter {
                            lines: display_lines,
                            theme,
                            galley: galley.clone(),
                        });
                        galley
                    }
                };
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
                let job = match cache
                    .viewer
                    .as_ref()
                    .filter(|c| c.theme == theme && c.text == text)
                    .map(|c| c.job.clone())
                {
                    Some(job) => job,
                    None => {
                        let job = highlight_job(text, font_id.clone(), default_color, &pal);
                        cache.viewer = Some(CachedJob {
                            text: text.to_string(),
                            theme,
                            job: job.clone(),
                        });
                        job
                    }
                };
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

    /// Headless check that the cached editor galley really contains the full
    /// source after several frames (i.e. the cache never serves a stale or
    /// partial galley).
    #[test]
    fn editor_cache_holds_full_galley() {
        let ctx = Context::default();
        let mut cache = HighlightCache::default();
        let mut text = crate::presets::ALL[0].code.to_string();
        for _ in 0..3 {
            ctx.run_ui(bevy_egui::egui::RawInput::default(), |ui| {
                show_code_editor(ui, &mut text, Theme::Dark, &mut cache);
            })
            .drop_without_applying_deltas();
        }
        let cached = cache.editor.as_ref().expect("editor galley cached");
        assert_eq!(cached.galley.text(), text.as_str());
        assert!(cached.galley.rows.len() > 10);
        assert!(cache.editor_gutter.is_some());
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
        assert!(cache.viewer_gutter.is_some());
    }
}
