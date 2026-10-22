//! A code editor for egui with syntax highlighting.
//!
//! egui has no built-in code editor, so this composes two widgets:
//!
//! 1. a [`LayoutJob`] galley produced by [`crate::highlight`] is painted at
//!    the exact position where the text edit will lay out its text, and
//! 2. an [`egui::TextEdit`] renders on top with a fully transparent text
//!    color, providing the cursor, selection, IME and editing behavior.
//!
//! Because both layers use the same `FontId`, the same wrap width
//! (`f32::INFINITY`, i.e. no wrapping) and the same margin, the glyphs line up
//! pixel-perfectly. A line-number gutter is painted as a second galley with
//! the same row height.

use bevy_egui::egui::text::CCursor;
use bevy_egui::egui::text::LayoutJob;
use bevy_egui::egui::text_selection::CCursorRange;
use bevy_egui::egui::widgets::text_edit::TextEditState;
use bevy_egui::egui::{
    Align, Color32, Context, FontId, Frame, Id, Key, Label, Layout, Margin, Modifiers, ScrollArea,
    Sense, TextEdit, Ui, Vec2, pos2,
};

use crate::highlight::highlight_job;

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

/// Show the highlighted code editor inside `ui`.
pub fn show_code_editor(ui: &mut Ui, text: &mut String) -> EditorOutput {
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

                // --- line-number gutter ---
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
                let gutter_galley = ctx.fonts_mut(|f| f.layout_job(gutter_job));
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

                // --- highlighted text behind the text edit ---
                let job = highlight_job(text, font_id.clone(), default_color);
                let galley = ctx.fonts_mut(|f| f.layout_job(job));
                let text_pos = ui.cursor().min + Vec2::new(margin.left as f32, margin.top as f32);
                ui.painter().galley(text_pos, galley, default_color);

                // --- the actual (transparent-text) editor on top ---
                let response = ui.add(
                    TextEdit::multiline(text)
                        .font(font_id.clone())
                        .margin(margin)
                        .desired_width(f32::INFINITY)
                        .frame(Frame::NONE)
                        .text_color(Color32::TRANSPARENT),
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
/// editor, but the text is painted as selectable [`Label`] galleys instead of
/// a `TextEdit`, since there is nothing to edit.
pub fn show_code_view(ui: &mut Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let font_id = FontId::monospace(13.0);
    let default_color = ui.visuals().text_color();

    let display_lines = text.matches('\n').count() + 1;
    let gutter_text: String = (1..=display_lines).map(|i| i.to_string()).collect();
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
                // --- line-number gutter (same painted galley as the editor) ---
                let mut gutter_job = LayoutJob::simple(
                    gutter_text,
                    font_id.clone(),
                    ui.visuals().weak_text_color(),
                    f32::INFINITY,
                );
                gutter_job.wrap.max_width = f32::INFINITY;
                let gutter_galley = ctx.fonts_mut(|f| f.layout_job(gutter_job));
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
                let job = highlight_job(text, font_id.clone(), default_color);
                ui.add(Label::new(job).selectable(true));
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

/// egui keeps these functions private to `text_cursor_state`; ours are tiny.
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
