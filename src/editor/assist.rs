//! Code assistance in the editor: the completion and signature-help popups,
//! go to definition, formatting and type hints.
//!
//! The editor drives an [`Assistant`] (wgsl-analyzer in the app, a script in
//! the tests): it asks at the cursor and shows the answer whose ticket it
//! holds. Popup keys are taken from the input before the `TextEdit` sees
//! them ([`before`]), and only while a popup is up; requests and painting
//! happen after it, from the text and galley it just produced ([`after`]).

use std::ops::Range;

use bevy::prelude::Resource;
use bevy_egui::egui::text::{CCursor, LayoutJob, TextFormat};
use bevy_egui::egui::text_selection::CCursorRange;
use bevy_egui::egui::widgets::text_edit::{TextEditOutput, TextEditState};
use bevy_egui::egui::{
    Align, Align2, Area, Color32, Context, Event, EventFilter, FontId, Frame, Id, Key, Modifiers,
    Order, Rect, ScrollArea, Stroke, Ui, Vec2, pos2,
};

use super::byte_index_from_char_index;
use crate::analyzer::{
    Analyzer, Answer, CompletionItem, CompletionKind, Definition, Diagnostic, FormatError,
    HintKind, InlayHint, Signature, Ticket,
};
use crate::highlight::{is_ident_char, is_ident_start};
use crate::theme::SYNTAX;

/// What the editor asks of a language server.
pub trait Assistant {
    fn diagnostics(&self) -> &[Diagnostic];
    fn inlay_hints(&self) -> &[InlayHint];
    fn completion(&self) -> Option<&Answer<Vec<CompletionItem>>>;
    fn signature(&self) -> Option<&Answer<Option<Signature>>>;
    fn take_definition(&mut self) -> Option<Answer<Option<Definition>>>;
    fn take_format(&mut self) -> Option<Answer<Result<String, FormatError>>>;
    fn request_completion(&mut self, text: &str, at: usize) -> Option<Ticket>;
    fn request_signature(&mut self, text: &str, at: usize) -> Option<Ticket>;
    fn request_definition(&mut self, text: &str, at: usize) -> Option<Ticket>;
    fn request_format(&mut self, text: &str, selection: Option<Range<usize>>) -> Option<Ticket>;
}

impl Assistant for Analyzer {
    fn diagnostics(&self) -> &[Diagnostic] {
        Analyzer::diagnostics(self)
    }
    fn inlay_hints(&self) -> &[InlayHint] {
        Analyzer::inlay_hints(self)
    }
    fn completion(&self) -> Option<&Answer<Vec<CompletionItem>>> {
        Analyzer::completion(self)
    }
    fn signature(&self) -> Option<&Answer<Option<Signature>>> {
        Analyzer::signature(self)
    }
    fn take_definition(&mut self) -> Option<Answer<Option<Definition>>> {
        Analyzer::take_definition(self)
    }
    fn take_format(&mut self) -> Option<Answer<Result<String, FormatError>>> {
        Analyzer::take_format(self)
    }
    fn request_completion(&mut self, text: &str, at: usize) -> Option<Ticket> {
        Analyzer::request_completion(self, text, at)
    }
    fn request_signature(&mut self, text: &str, at: usize) -> Option<Ticket> {
        Analyzer::request_signature(self, text, at)
    }
    fn request_definition(&mut self, text: &str, at: usize) -> Option<Ticket> {
        Analyzer::request_definition(self, text, at)
    }
    fn request_format(&mut self, text: &str, selection: Option<Range<usize>>) -> Option<Ticket> {
        Analyzer::request_format(self, text, selection)
    }
}

/// Diagnostics alone, nothing to ask (the editor's own tests).
#[cfg(test)]
pub struct DiagnosticsOnly<'a>(pub &'a [Diagnostic]);

#[cfg(test)]
impl Assistant for DiagnosticsOnly<'_> {
    fn diagnostics(&self) -> &[Diagnostic] {
        self.0
    }
    fn inlay_hints(&self) -> &[InlayHint] {
        &[]
    }
    fn completion(&self) -> Option<&Answer<Vec<CompletionItem>>> {
        None
    }
    fn signature(&self) -> Option<&Answer<Option<Signature>>> {
        None
    }
    fn take_definition(&mut self) -> Option<Answer<Option<Definition>>> {
        None
    }
    fn take_format(&mut self) -> Option<Answer<Result<String, FormatError>>> {
        None
    }
    fn request_completion(&mut self, _: &str, _: usize) -> Option<Ticket> {
        None
    }
    fn request_signature(&mut self, _: &str, _: usize) -> Option<Ticket> {
        None
    }
    fn request_definition(&mut self, _: &str, _: usize) -> Option<Ticket> {
        None
    }
    fn request_format(&mut self, _: &str, _: Option<Range<usize>>) -> Option<Ticket> {
        None
    }
}

/// Which assistance is switched on (from the settings).
#[derive(Clone, Copy, Debug)]
pub struct AssistOptions {
    /// Open the completion list while typing, not only on Ctrl+Space.
    pub autocomplete: bool,
    /// Show the types of `let`/`var` at the end of their line.
    pub type_hints: bool,
}

impl Default for AssistOptions {
    fn default() -> Self {
        Self {
            autocomplete: true,
            type_hints: true,
        }
    }
}

/// What the editor needs the app to do.
#[derive(Debug, Default, PartialEq)]
pub struct AssistOutput {
    /// A definition outside the editor text, to show read-only.
    pub peek: Option<Definition>,
    /// Formatting was asked for, but gave no text.
    pub format_failed: Option<FormatError>,
}

/// The editor's popups and the answers it waits for.
#[derive(Resource, Default)]
pub struct CodeAssist {
    pub(super) completion: Option<CompletionPopup>,
    pub(super) signature: Option<SignaturePopup>,
    definition: Option<Ticket>,
    format: Option<Ticket>,
    /// The format button was pressed; asked on the next frame.
    format_requested: bool,
    /// A char to bring into view once the galley is laid out.
    scroll_to: Option<usize>,
}

impl CodeAssist {
    /// Format the editor text (or its selection) on the next frame.
    pub fn request_format(&mut self) {
        self.format_requested = true;
    }
}

pub(super) struct CompletionPopup {
    /// Where the word being completed starts: the list is the scope there,
    /// filtered by what is typed from here to the cursor.
    anchor: usize,
    ticket: Option<Ticket>,
    selected: usize,
    /// The word the selection was made for; a new word selects the top.
    word: String,
    /// Where the list was painted, so a click on it is not taken for a
    /// click elsewhere.
    rect: Option<Rect>,
}

pub(super) struct SignaturePopup {
    /// Answers to tickets from here on belong to this popup.
    since: Ticket,
    /// The cursor it was last asked for.
    at: usize,
}

/// The rows to show for `word`: items whose filter text starts with it
/// (ignoring case) first, then those containing it, each group in the
/// server's order.
pub fn filtered<'a>(items: &'a [CompletionItem], word: &str) -> Vec<&'a CompletionItem> {
    let word = word.to_lowercase();
    let mut ranked: Vec<(u8, &CompletionItem)> = items
        .iter()
        .filter_map(|item| {
            let filter = item.filter.to_lowercase();
            if filter.starts_with(&word) {
                Some((0, item))
            } else if filter.contains(&word) {
                Some((1, item))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by(|(ra, a), (rb, b)| (ra, &a.sort, &a.label).cmp(&(rb, &b.sort, &b.label)));
    // An imported function also comes as its import's name: one row each.
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .map(|(_, item)| item)
        .filter(|item| seen.insert(item.label.as_str()))
        .collect()
}

/// Where the identifier ending at char `cursor` starts.
fn word_start(chars: &[char], cursor: usize) -> usize {
    let cursor = cursor.min(chars.len());
    cursor
        - chars[..cursor]
            .iter()
            .rev()
            .take_while(|c| is_ident_char(**c))
            .count()
}

/// The single cursor (no selection) of `state`, as a char index.
fn caret(state: &TextEditState) -> Option<usize> {
    let range = state.cursor.char_range()?;
    range.is_empty().then_some(range.primary.index.0)
}

/// The popup's rows for the current text, if it should show.
fn completion_rows<'a>(
    popup: &CompletionPopup,
    assistant: &'a dyn Assistant,
    chars: &[char],
    cursor: usize,
) -> Option<Vec<&'a CompletionItem>> {
    let answer = assistant.completion()?;
    if Some(answer.ticket) != popup.ticket || cursor < popup.anchor || cursor > chars.len() {
        return None;
    }
    let word: String = chars[popup.anchor..cursor].iter().collect();
    let rows = filtered(&answer.value, &word);
    (!rows.is_empty()).then_some(rows)
}

/// Make `text` (with `state`'s cursor) an undo point before an edit made
/// outside the TextEdit: egui only records states while the editor is
/// focused and has rested a second, so a format from the button, or a
/// completion right after typing, would otherwise not be undoable as such.
fn remember_for_undo(state: &mut TextEditState, text: &str) {
    let mut undoer = state.undoer();
    let cursor = state.cursor.char_range().unwrap_or_default();
    undoer.add_undo(&(cursor, text.to_owned()));
    state.set_undoer(undoer);
}

/// Replace the typed word with `item`, leaving the cursor after it.
fn accept(
    ctx: &Context,
    id: Id,
    text: &mut String,
    state: &mut TextEditState,
    anchor: usize,
    cursor: usize,
    item: &CompletionItem,
) {
    remember_for_undo(state, text);
    // The server may replace more than the word (a whole `a::b::c` path):
    // text before the anchor is as it was when it answered.
    let anchor = item.replace_from.map_or(anchor, |from| from.min(anchor));
    let (from, to) = (
        byte_index_from_char_index(text, anchor),
        byte_index_from_char_index(text, cursor),
    );
    text.replace_range(from..to, &item.insert);
    let after = anchor + item.insert.chars().count();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(after))));
    state.clone().store(ctx, id);
}

/// Before the `TextEdit` runs: apply answers that change the text or the
/// cursor, and take the keys an open popup uses.
pub fn before(
    ctx: &Context,
    id: Id,
    focused: bool,
    text: &mut String,
    assist: &mut CodeAssist,
    assistant: &mut dyn Assistant,
    out: &mut AssistOutput,
) {
    let Some(mut state) = TextEditState::load(ctx, id) else {
        return;
    };

    if let Some(answer) = assistant.take_format()
        && Some(answer.ticket) == assist.format
    {
        assist.format = None;
        match answer.value {
            Ok(formatted) if formatted != *text => {
                let cursor = state.cursor.char_range().map_or(0, |r| r.primary.index.0);
                let cursor = follow(text, &formatted, cursor);
                remember_for_undo(&mut state, text);
                *text = formatted;
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
                state.clone().store(ctx, id);
                assist.completion = None;
                assist.signature = None;
            }
            Ok(_) => {}
            Err(error) => out.format_failed = Some(error),
        }
    }

    if let Some(answer) = assistant.take_definition()
        && Some(answer.ticket) == assist.definition
    {
        assist.definition = None;
        match answer.value {
            Some(Definition::User(range)) => {
                state.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(range.start),
                    CCursor::new(range.end),
                )));
                state.clone().store(ctx, id);
                assist.scroll_to = Some(range.start);
            }
            Some(module) => out.peek = Some(module),
            None => {}
        }
    }

    if !focused {
        return;
    }
    let chars: Vec<char> = text.chars().collect();
    let cursor = caret(&state);

    // The completion list's keys, while it shows.
    let rows = assist
        .completion
        .as_ref()
        .zip(cursor)
        .and_then(|(popup, cursor)| completion_rows(popup, assistant, &chars, cursor));
    if let (Some(rows), Some(popup), Some(cursor)) = (rows, assist.completion.as_mut(), cursor) {
        let count = rows.len();
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if pressed(Key::ArrowDown) {
            popup.selected = (popup.selected + 1) % count;
        }
        if pressed(Key::ArrowUp) {
            popup.selected = (popup.selected + count - 1) % count;
        }
        if pressed(Key::Escape) {
            assist.completion = None;
        } else if pressed(Key::Enter) || pressed(Key::Tab) {
            let item = rows[popup.selected.min(count - 1)].clone();
            let anchor = popup.anchor;
            assist.completion = None;
            accept(ctx, id, text, &mut state, anchor, cursor, &item);
        }
    } else if assist.signature.is_some()
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    {
        assist.signature = None;
    }
}

/// Where char `cursor` of `old` lands in `new`: before the changed span it
/// stays, after it it moves by the length change, inside it it goes to the
/// end of the new span.
fn follow(old: &str, new: &str, cursor: usize) -> usize {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
    match cursor {
        c if c <= prefix => c,
        c if c >= old_end => c - old_end + new_end,
        _ => new_end,
    }
}

/// The keys that ask something at the cursor, read before the `TextEdit`
/// (which would type or act on some of them).
#[derive(Clone, Copy, Debug, Default)]
pub struct Shortcuts {
    complete: bool,
    definition: bool,
    format: bool,
}

/// Take Ctrl+Space (completion), F12 (go to definition) and Shift+Alt+F
/// (format) from the input of a focused editor.
pub fn shortcuts(ctx: &Context, focused: bool) -> Shortcuts {
    if !focused {
        return Shortcuts::default();
    }
    ctx.input_mut(|i| {
        let complete = i.consume_key(Modifiers::COMMAND, Key::Space);
        if complete {
            // Some platforms also send the space as text.
            i.events
                .retain(|e| !matches!(e, Event::Text(t) if t == " "));
        }
        let format = i.consume_key(Modifiers::SHIFT | Modifiers::ALT, Key::F);
        if format {
            // On macOS, Shift+Option+F also types `Ï`.
            i.events.retain(|e| !matches!(e, Event::Text(_)));
        }
        Shortcuts {
            complete,
            definition: i.consume_key(Modifiers::NONE, Key::F12),
            format,
        }
    })
}

/// The text typed into the editor this frame.
pub fn typed(ctx: &Context, focused: bool) -> String {
    if !focused {
        return String::new();
    }
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    })
}

/// Whether char `cursor` sits in a `//` comment.
fn in_comment(chars: &[char], cursor: usize) -> bool {
    let line_start = chars[..cursor]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    chars[line_start..cursor]
        .windows(2)
        .any(|w| w == ['/', '/'])
}

/// After the `TextEdit` ran: ask what the typing and keys call for, keep
/// the popups in step with the cursor, and paint them.
#[expect(clippy::too_many_arguments, reason = "one call site, the editor")]
pub fn after(
    ui: &Ui,
    output: &TextEditOutput,
    text: &mut String,
    typed: &str,
    keys: Shortcuts,
    assist: &mut CodeAssist,
    assistant: &mut dyn Assistant,
    options: AssistOptions,
) {
    let ctx = ui.ctx();
    let id = output.response.response.id;
    let chars: Vec<char> = text.chars().collect();
    let state = &output.state;
    let cursor = caret(state);
    let focused = ctx.memory(|m| m.has_focus(id));

    // --- requests ---------------------------------------------------------
    let last = typed.chars().last();
    if let Some(cursor) = cursor {
        let word_from = word_start(&chars, cursor);
        let word_ok = word_from < cursor && is_ident_start(chars[word_from]);
        let member = last == Some('.')
            && cursor >= 2
            && (is_ident_char(chars[cursor - 2]) || matches!(chars[cursor - 2], ')' | ']'))
            && {
                let before = word_start(&chars, cursor - 1);
                before == cursor - 1 || is_ident_start(chars[before])
            };
        let open_at = if keys.complete {
            Some(if word_ok { word_from } else { cursor })
        } else if !options.autocomplete || in_comment(&chars, cursor) {
            None
        } else if member {
            Some(cursor)
        } else if last.is_some_and(is_ident_char) && word_ok && assist.completion.is_none() {
            Some(word_from)
        } else {
            None
        };
        if let Some(anchor) = open_at {
            assist.completion = Some(CompletionPopup {
                anchor,
                ticket: assistant.request_completion(text, anchor),
                selected: 0,
                word: String::new(),
                rect: None,
            });
        }

        match last {
            Some('(' | ',') => {
                if let Some(since) = assistant.request_signature(text, cursor) {
                    assist.signature = Some(SignaturePopup { since, at: cursor });
                }
            }
            Some(')') => assist.signature = None,
            _ => {
                // Moved within the call: ask again (the answer is `None`
                // once the cursor left it, which closes the popup).
                if let Some(popup) = &mut assist.signature
                    && popup.at != cursor
                {
                    popup.at = cursor;
                    assistant.request_signature(text, cursor);
                }
            }
        }

        let clicked = output.response.response.clicked() && ui.input(|i| i.modifiers.command);
        if (keys.definition || clicked)
            && let Some(ticket) = assistant.request_definition(text, cursor)
        {
            assist.definition = Some(ticket);
        }
    }
    if keys.format || std::mem::take(&mut assist.format_requested) {
        let selection = state.cursor.char_range().and_then(|r| {
            let [a, b] = r.sorted_cursors();
            (a.index != b.index).then_some(a.index.0..b.index.0)
        });
        assist.format = assistant.request_format(text, selection);
    }

    // --- painting ---------------------------------------------------------
    let origin = output.galley_pos - Vec2::new(output.galley.rect.left(), 0.0);
    if options.type_hints {
        paint_type_hints(ui, output, origin, &chars, assistant.inlay_hints());
    }
    let cursor_rect = |at: usize| {
        output
            .galley
            .pos_from_cursor(CCursor::new(at.min(chars.len())))
            .translate(origin.to_vec2())
    };
    if let Some(at) = assist.scroll_to.take() {
        ui.scroll_to_rect(cursor_rect(at).expand(24.0), Some(Align::Center));
    }

    if let Some(cursor) = cursor
        && let Some(popup) = &assist.signature
        && let Some(answer) = assistant.signature()
        && answer.ticket >= popup.since
    {
        match &answer.value {
            Some(signature) => paint_signature(ctx, cursor_rect(cursor), signature),
            // The latest answer says the cursor is not in a call.
            None => assist.signature = None,
        }
    }

    let mut accepted = None;
    let mut keep = false;
    if let (Some(popup), Some(cursor)) = (&mut assist.completion, cursor) {
        // Still on the word it was opened for.
        let on_word = cursor >= popup.anchor
            && cursor <= chars.len()
            && chars[popup.anchor..cursor]
                .iter()
                .all(|c| is_ident_char(*c));
        let pointer_on_list = popup
            .rect
            .zip(ctx.pointer_interact_pos())
            .is_some_and(|(r, p)| r.contains(p));
        keep = on_word && (focused || pointer_on_list);
        if keep && let Some(rows) = completion_rows(popup, assistant, &chars, cursor) {
            let word: String = chars[popup.anchor..cursor].iter().collect();
            if word != popup.word {
                popup.word = word;
                popup.selected = 0;
            }
            popup.selected = popup.selected.min(rows.len() - 1);
            let at = cursor_rect(popup.anchor);
            let (rect, clicked) = paint_completion(ctx, at, &rows, popup.selected);
            popup.rect = Some(rect);
            if let Some(i) = clicked {
                accepted = Some((popup.anchor, cursor, rows[i].clone()));
            }
        } else {
            popup.rect = None;
        }
    }
    if !keep || accepted.is_some() {
        assist.completion = None;
    }
    if !focused && accepted.is_none() {
        assist.signature = None;
    }
    if let Some((anchor, cursor, item)) = accepted {
        let mut state = output.state.clone();
        accept(ctx, id, text, &mut state, anchor, cursor, &item);
        ctx.memory_mut(|m| m.request_focus(id));
    }
    // egui drops focus on Escape before any widget runs, unless the focused
    // one claims the key: while a popup is up, Escape closes it instead.
    // (Set after the TextEdit, which sets its own filter every frame.)
    if focused && (assist.completion.is_some() || assist.signature.is_some()) {
        let filter = EventFilter {
            tab: true,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: true,
        };
        ctx.memory_mut(|m| m.set_focus_lock_filter(id, filter));
    }
}

/// Room between a line's code and its type hints.
const HINT_GAP: f32 = 24.0;

/// The type hints of each row (`st: vec2<f32>, t: f32`), faded, after the
/// row's last glyph: inside the line they would shift every char the
/// TextEdit's cursor maps.
fn paint_type_hints(
    ui: &Ui,
    output: &TextEditOutput,
    origin: bevy_egui::egui::Pos2,
    chars: &[char],
    hints: &[InlayHint],
) {
    let galley = &output.galley;
    // In text order, so each row's hints gather into one line.
    let mut hints: Vec<&InlayHint> = hints.iter().collect();
    hints.sort_by_key(|h| h.at);
    let mut rows: Vec<(usize, String)> = Vec::new();
    for hint in hints {
        // `[error]`: the server could not type it (code mid-edit).
        if hint.kind != HintKind::Type || hint.at > chars.len() || hint.label.contains("[error]") {
            continue;
        }
        // `: f32` sits right after the name it types.
        let name: String = chars[word_start(chars, hint.at)..hint.at].iter().collect();
        let label = hint.label.trim();
        let text = if label.starts_with(':') {
            format!("{name}{label}")
        } else {
            format!("{name}: {label}")
        };
        let row = galley.layout_from_cursor(CCursor::new(hint.at)).row;
        match rows.last_mut() {
            Some((r, line)) if *r == row => {
                line.push_str(", ");
                line.push_str(&text);
            }
            _ => rows.push((row, text)),
        }
    }
    let font = FontId::monospace(13.0);
    let color = ui.visuals().weak_text_color().gamma_multiply(0.7);
    let painter = ui.painter();
    for (row, text) in rows {
        let Some(placed) = galley.rows.get(row) else {
            continue;
        };
        let at = origin + placed.pos.to_vec2() + Vec2::new(placed.row.size.x + HINT_GAP, 0.0);
        painter.text(at, Align2::LEFT_TOP, text, font.clone(), color);
    }
}

/// The color a completion row's label gets.
fn kind_color(kind: CompletionKind, ui: &Ui) -> Color32 {
    match kind {
        CompletionKind::Function => SYNTAX.builtin,
        CompletionKind::Constant => SYNTAX.number,
        CompletionKind::Field => SYNTAX.attribute,
        CompletionKind::Type => SYNTAX.ty,
        CompletionKind::Keyword => SYNTAX.keyword,
        CompletionKind::Variable | CompletionKind::Other => ui.visuals().text_color(),
    }
}

/// The part of `item`'s detail its label does not already say:
/// `(v: vec3<f32>) -> f32` for `fn snoise3(v: vec3<f32>) -> f32`.
fn short_detail(item: &CompletionItem) -> &str {
    let Some(detail) = item.detail.as_deref() else {
        return "";
    };
    for keyword in [
        "fn ",
        "const ",
        "override ",
        "var ",
        "let ",
        "struct ",
        "alias ",
    ] {
        if let Some(rest) = detail
            .strip_prefix(keyword)
            .and_then(|d| d.strip_prefix(item.label.as_str()))
        {
            return rest.trim_start_matches(": ").trim_start();
        }
    }
    detail
}

/// Rows shown at once; the rest scroll.
const VISIBLE_ROWS: usize = 10;

/// The completion list under `at` (the word's first char); returns where it
/// was painted and the row clicked, if any.
fn paint_completion(
    ctx: &Context,
    at: Rect,
    rows: &[&CompletionItem],
    selected: usize,
) -> (Rect, Option<usize>) {
    let font = FontId::monospace(13.0);
    let mut clicked = None;
    let response = Area::new(Id::new("wesl_shadertoy_completion"))
        .order(Order::Foreground)
        .fixed_pos(at.left_bottom() + Vec2::new(-4.0, 2.0))
        .constrain(true)
        .show(ctx, |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                let row_height = ui.fonts_mut(|f| f.row_height(&font)) + 4.0;
                ScrollArea::vertical()
                    .max_height(row_height * VISIBLE_ROWS as f32)
                    .show(ui, |ui| {
                        for (i, item) in rows.iter().enumerate() {
                            // On the selection fill, the theme's selected
                            // text color reads; the tints do not.
                            let on = ui.visuals().selection.stroke.color;
                            let (label, detail) = if i == selected {
                                (on, on.gamma_multiply(0.8))
                            } else {
                                (kind_color(item.kind, ui), ui.visuals().weak_text_color())
                            };
                            let mut job = LayoutJob::default();
                            job.append(&item.label, 0.0, TextFormat::simple(font.clone(), label));
                            job.append(
                                short_detail(item),
                                12.0,
                                TextFormat::simple(font.clone(), detail),
                            );
                            let row = ui.selectable_label(i == selected, job);
                            if i == selected {
                                row.scroll_to_me(None);
                            }
                            if row.clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                // The whole signature of the selected function, which the
                // row may only show the end of.
                if let Some(detail) = rows[selected].detail.as_deref()
                    && detail.starts_with("fn ")
                {
                    ui.separator();
                    ui.label(egui_text(detail, &font, ui.visuals().weak_text_color()));
                }
            });
        });
    (response.response.rect, clicked)
}

fn egui_text(text: &str, font: &FontId, color: Color32) -> LayoutJob {
    LayoutJob::simple_singleline(text.to_owned(), font.clone(), color)
}

/// The signature above the cursor's row, its active parameter underlined
/// in the strong text color.
fn paint_signature(ctx: &Context, cursor: Rect, signature: &Signature) {
    let font = FontId::monospace(13.0);
    Area::new(Id::new("wesl_shadertoy_signature"))
        .order(Order::Foreground)
        .fixed_pos(pos2(cursor.left() - 4.0, cursor.top() - 4.0))
        .pivot(Align2::LEFT_BOTTOM)
        .constrain(true)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                let weak = ui.visuals().weak_text_color();
                let strong = ui.visuals().strong_text_color();
                let active = signature.active.and_then(|a| signature.params.get(a));
                let chars: Vec<char> = signature.label.chars().collect();
                let mut job = LayoutJob::default();
                let mut push = |range: Range<usize>, on: bool| {
                    let text: String = chars[range].iter().collect();
                    let mut format =
                        TextFormat::simple(font.clone(), if on { strong } else { weak });
                    if on {
                        format.underline = Stroke::new(1.0, strong);
                    }
                    job.append(&text, 0.0, format);
                };
                match active {
                    Some(r) if r.end <= chars.len() && r.start <= r.end => {
                        push(0..r.start, false);
                        push(r.clone(), true);
                        push(r.end..chars.len(), false);
                    }
                    _ => push(0..chars.len(), false),
                }
                ui.label(job);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, sort: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            kind: CompletionKind::Function,
            detail: None,
            insert: label.into(),
            replace_from: None,
            filter: label.into(),
            sort: sort.into(),
        }
    }

    #[test]
    fn filtering_ranks_prefixes_first() {
        let items = [
            item("i_resolution", "1"),
            item("snoise3", "2"),
            item("random22", "3"),
            item("Snoise2", "4"),
            // The import of `snoise3`, under the same name.
            item("snoise3", "5"),
        ];
        let labels = |word| -> Vec<&str> {
            filtered(&items, word)
                .iter()
                .map(|i| i.label.as_str())
                .collect()
        };
        assert_eq!(labels("sn"), ["snoise3", "Snoise2"]);
        // No prefix match: every label containing it, in the server's order.
        assert_eq!(
            labels("o"),
            ["i_resolution", "snoise3", "random22", "Snoise2"]
        );
        assert_eq!(labels("2"), ["random22", "Snoise2"]);
        assert_eq!(labels("").len(), 4);
        assert!(labels("xyz").is_empty());
    }

    #[test]
    fn words_and_comments() {
        let chars: Vec<char> = "let n = snoi".chars().collect();
        assert_eq!(word_start(&chars, chars.len()), 8);
        assert_eq!(word_start(&chars, 7), 7);
        let chars: Vec<char> = "x // sno\ny".chars().collect();
        assert!(in_comment(&chars, 8));
        assert!(!in_comment(&chars, 10));
    }

    #[test]
    fn details_drop_what_the_label_says() {
        let mut it = item("snoise3", "");
        it.detail = Some("fn snoise3(v: vec3<f32>) -> f32".into());
        assert_eq!(short_detail(&it), "(v: vec3<f32>) -> f32");
        it.detail = Some("const snoise3: f32".into());
        assert_eq!(short_detail(&it), "f32");
        it.detail = Some("vec2<f32>".into());
        assert_eq!(short_detail(&it), "vec2<f32>");
    }

    #[test]
    fn cursor_follows_formatting() {
        let old = "fn f(){return;}\nlet x = 1;";
        let new = "fn f() {\n    return;\n}\nlet x = 1;";
        // After the changed span: same distance from the end.
        let x = old.find('x').unwrap();
        assert_eq!(follow(old, new, x), new.find('x').unwrap());
        // Before it: unchanged.
        assert_eq!(follow(old, new, 2), 2);
    }
}
