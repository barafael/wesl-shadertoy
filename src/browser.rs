//! The Examples window: every built-in example as a card with a live
//! thumbnail, filtered by a search box and tags; clicking a card opens it.
//!
//! The thumbnails are real renders ([`crate::render::previews`]). Each
//! example is compiled the first time its card scrolls into view (within a
//! small time budget per frame, so opening the window never stalls), into a
//! shader asset of its own that the render world builds pipelines for. Each
//! frame [`update_previews`] then asks for the hovered card plus a few of
//! the visible ones in turn, so a full window costs a handful of small
//! draws per frame. Hovering a card drives its `i_mouse` as a drag, so
//! interactive examples react in place.

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::examples::{self, Example};
use crate::render::ShaderToyUniform;
use crate::render::previews::{
    PREVIEW_FORMAT, PREVIEW_SIZE, PreviewFeedback, PreviewJob, PreviewJobs,
};
use crate::tweak::MAX_TWEAKS;

/// Narrowest thumbnail on screen, in points; cards grow to fill the row
/// (the image itself is always [`PREVIEW_SIZE`]).
const MIN_THUMB_WIDTH: f32 = 200.0;

/// Gap between cards, in points.
const GAP: f32 = 12.0;

/// Visible cards drawn per frame besides the hovered one, round-robin.
const BUDGET: usize = 4;

/// Time per frame spent compiling examples once at least one was compiled.
const COMPILE_BUDGET_MS: f32 = 8.0;

/// The browser's state: per-card resources and the filter.
#[derive(Resource)]
pub struct Browser {
    cards: Vec<Card>,
    query: String,
    tag: Option<&'static str>,
    /// Cards on screen in the last frame the window was shown.
    visible: Vec<usize>,
    /// The hovered card, with the pointer on its thumbnail and where the
    /// hover began (pixels, origin bottom-left).
    hovered: Option<(usize, Vec2, Vec2)>,
    /// `hovered` as of the frame before, where a continuing hover finds
    /// its start.
    last_hovered: Option<(usize, Vec2, Vec2)>,
    /// Round-robin cursor into `visible`.
    next: usize,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            cards: examples::ALL.iter().map(|_| Card::default()).collect(),
            query: String::new(),
            tag: None,
            visible: Vec::new(),
            hovered: None,
            last_hovered: None,
            next: 0,
        }
    }
}

#[derive(Default)]
struct Card {
    image: Option<(Handle<Image>, egui::TextureId)>,
    compiled: Option<Result<Compiled, String>>,
    /// `i_time` when it was last drawn, for its `i_time_delta`.
    last_drawn: f32,
}

struct Compiled {
    shader: Handle<Shader>,
    buffers: u8,
    /// The example's constants at their defaults, slot = index.
    tweaks: [Vec4; MAX_TWEAKS],
}

impl Browser {
    /// Call every frame before [`show`]: a closed window shows no cards.
    pub fn begin_frame(&mut self) {
        self.visible.clear();
        self.last_hovered = self.hovered.take();
    }
}

/// Allocate images and compile examples for the cards the window showed
/// last frame, and pick this frame's [`PreviewJobs`].
pub fn update_previews(
    mut browser: ResMut<Browser>,
    mut contexts: EguiContexts,
    mut images: ResMut<Assets<Image>>,
    mut shaders: ResMut<Assets<Shader>>,
    mut jobs: ResMut<PreviewJobs>,
    time: Res<Time>,
) {
    jobs.0.clear();
    let b = &mut *browser;
    if b.visible.is_empty() {
        return;
    }

    let started = Instant::now();
    let mut compiled_one = false;
    for &i in &b.visible {
        let card = &mut b.cards[i];
        if card.image.is_none() {
            let image = images.add(Image::new_target_texture(
                PREVIEW_SIZE.x,
                PREVIEW_SIZE.y,
                PREVIEW_FORMAT,
                None,
            ));
            let texture = contexts.add_image(bevy_egui::EguiTextureHandle::Strong(image.clone()));
            card.image = Some((image, texture));
        }
        let over_budget = started.elapsed().as_secs_f32() * 1000.0 > COMPILE_BUDGET_MS;
        if card.compiled.is_none() && !(compiled_one && over_budget) {
            card.compiled = Some(compile(i, &mut shaders));
            compiled_one = true;
        }
    }

    // The hovered card every frame, then the next few visible ones.
    let drawable = |b: &Browser, i: usize| matches!(b.cards[i].compiled, Some(Ok(_)));
    let mut picked: Vec<usize> = b.hovered.map(|(i, ..)| i).into_iter().collect();
    let candidates: Vec<usize> = b
        .visible
        .iter()
        .copied()
        .filter(|&i| drawable(b, i) && !picked.contains(&i))
        .collect();
    if !candidates.is_empty() {
        for k in 0..BUDGET.min(candidates.len()) {
            picked.push(candidates[(b.next + k) % candidates.len()]);
        }
        b.next = (b.next + BUDGET) % candidates.len();
    }

    let now = time.elapsed_secs();
    for i in picked {
        let hovered = b.hovered;
        let card = &mut b.cards[i];
        let (Some((image, _)), Some(Ok(compiled))) = (&card.image, &card.compiled) else {
            continue;
        };
        let mouse = match hovered {
            Some((h, pos, start)) if h == i => Vec4::new(pos.x, pos.y, start.x, start.y),
            _ => Vec4::ZERO,
        };
        let size = Vec4::new(PREVIEW_SIZE.x as f32, PREVIEW_SIZE.y as f32, 0.0, 0.0);
        let defined = |c: usize| compiled.buffers & (1 << c) != 0;
        let uniform = ShaderToyUniform {
            time: now,
            time_delta: (now - card.last_drawn).clamp(0.0, 0.1),
            resolution: Vec4::new(size.x, size.y, 1.0, 0.0),
            mouse,
            date: crate::date_uniforms(),
            sample_rate: crate::SAMPLE_RATE_HZ,
            frame_rate: 1.0 / time.delta_secs().max(1e-3),
            channel_time: Vec4::from(std::array::from_fn::<f32, 4, _>(|c| {
                if defined(c) { now } else { 0.0 }
            })),
            channel_resolution: std::array::from_fn(|c| if defined(c) { size } else { Vec4::ZERO }),
            tweaks: compiled.tweaks,
            ..default()
        };
        card.last_drawn = now;
        jobs.0.push(PreviewJob {
            card: i,
            image: image.id(),
            shader: compiled.shader.clone(),
            buffers: compiled.buffers,
            uniform,
        });
    }
}

/// Compile example `i` into a shader asset of its own.
fn compile(i: usize, shaders: &mut Assets<Shader>) -> Result<Compiled, String> {
    let example = &examples::ALL[i];
    let compiled = crate::compile::compile_user_shader(example.code).map_err(|e| {
        warn!("example {} does not compile: {e}", example.name);
        e.to_string()
    })?;
    let mut tweaks = [Vec4::ZERO; MAX_TWEAKS];
    for (slot, c) in compiled.tweaks.iter().enumerate() {
        tweaks[slot] = Vec4::from_array(c.value);
    }
    let path = format!("shadertoy://example/{i}");
    Ok(Compiled {
        shader: shaders.add(Shader::from_wgsl(compiled.wgsl, path)),
        buffers: compiled.buffers,
        tweaks,
    })
}

/// The window's contents. `current` names the example in the editor (if
/// it is one); returns the example the user clicked.
pub fn show(
    ui: &mut egui::Ui,
    b: &mut Browser,
    feedback: &PreviewFeedback,
    current: Option<&str>,
) -> Option<&'static Example> {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut b.query)
                .hint_text("search")
                .desired_width(160.0),
        );
        if ui
            .add_enabled(
                !b.query.is_empty() || b.tag.is_some(),
                egui::Button::new("clear"),
            )
            .clicked()
        {
            b.query.clear();
            b.tag = None;
        }
    });
    ui.horizontal_wrapped(|ui| {
        for tag in examples::tags() {
            let on = b.tag == Some(tag);
            if ui.selectable_label(on, tag).clicked() {
                b.tag = (!on).then_some(tag);
            }
        }
    });
    ui.separator();

    let query = b.query.to_lowercase();
    let matches = |e: &Example| {
        b.tag.is_none_or(|t| e.tags.contains(&t))
            && (query.is_empty()
                || [e.name, e.summary, e.author]
                    .iter()
                    .chain(e.tags)
                    .any(|s| s.to_lowercase().contains(&query)))
    };
    let shown: Vec<usize> = (0..examples::ALL.len())
        .filter(|&i| matches(&examples::ALL[i]))
        .collect();

    let mut clicked = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if shown.is_empty() {
                ui.weak("No example matches.");
            }
            let width = ui.available_width();
            let columns = (((width + GAP) / (MIN_THUMB_WIDTH + GAP)) as usize).max(1);
            let thumb_width = (width - GAP * (columns - 1) as f32) / columns as f32;
            let thumb = egui::vec2(thumb_width, thumb_width * 9.0 / 16.0);
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            for row in shown.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for &i in row {
                        if card(ui, b, i, thumb, feedback, current) {
                            clicked = Some(&examples::ALL[i]);
                        }
                    }
                });
            }
        });
    clicked
}

/// One card: thumbnail, name and summary. Returns whether it was clicked.
fn card(
    ui: &mut egui::Ui,
    b: &mut Browser,
    i: usize,
    thumb: egui::Vec2,
    feedback: &PreviewFeedback,
    current: Option<&str>,
) -> bool {
    let example = &examples::ALL[i];
    let layout = egui::Layout::top_down(egui::Align::LEFT);
    let response = ui
        .allocate_ui_with_layout(thumb, layout, |ui| {
            ui.set_width(thumb.x);
            ui.spacing_mut().item_spacing.y = 2.0;
            let (rect, response) = ui.allocate_exact_size(thumb, egui::Sense::click());
            if ui.is_rect_visible(rect) {
                b.visible.push(i);
                thumbnail(ui, b, i, rect, feedback);
            }
            let selected = current == Some(example.name);
            let stroke = if selected {
                ui.visuals().selection.stroke
            } else if response.hovered() {
                ui.visuals().widgets.hovered.fg_stroke
            } else {
                egui::Stroke::NONE
            };
            ui.painter()
                .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
            if let Some(pos) = response.hover_pos() {
                // Thumbnail pixels, origin bottom-left like i_mouse.
                let t = (pos - rect.min) / rect.size();
                let px = Vec2::new(
                    t.x * PREVIEW_SIZE.x as f32,
                    (1.0 - t.y) * PREVIEW_SIZE.y as f32,
                );
                let start = match b.last_hovered {
                    Some((h, _, start)) if h == i => start,
                    _ => px,
                };
                b.hovered = Some((i, px, start));
            }
            ui.add(egui::Label::new(egui::RichText::new(example.name).strong()).truncate());
            ui.add(
                egui::Label::new(egui::RichText::new(example.summary).small().weak()).truncate(),
            );
            response
        })
        .inner;
    response.on_hover_ui(|ui| details(ui, example)).clicked()
}

fn thumbnail(ui: &egui::Ui, b: &Browser, i: usize, rect: egui::Rect, feedback: &PreviewFeedback) {
    let painter = ui.painter();
    let card = &b.cards[i];
    match (&card.image, &card.compiled) {
        (_, Some(Err(_))) => {
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "does not compile",
                egui::FontId::proportional(12.0),
                crate::theme::ERR_COLOR,
            );
        }
        (Some((_, texture)), _) if feedback.drawn(i) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(*texture, rect, uv, egui::Color32::WHITE);
        }
        _ => {
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "compiling…",
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

/// The hover card: what it is, who made it, under which license.
fn details(ui: &mut egui::Ui, e: &Example) {
    ui.set_max_width(320.0);
    ui.strong(e.name);
    ui.label(e.summary);
    ui.separator();
    if !e.author.is_empty() {
        ui.label(format!("by {}", e.author));
    }
    if !e.source.is_empty() {
        ui.label(egui::RichText::new(e.source).small());
    }
    ui.label(egui::RichText::new(format!("license: {}", e.license)).small());
    if !e.tags.is_empty() {
        ui.label(egui::RichText::new(e.tags.join(" · ")).small().weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cards report which of them are on screen and hovered, the tag filter
    /// narrows them down, and a click on a thumbnail opens that example.
    #[test]
    fn cards_track_visibility_hover_and_clicks() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 500.0));
        let feedback = PreviewFeedback::default();
        let mut b = Browser::default();
        let run = |b: &mut Browser, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut clicked = None;
            ctx.run_ui(input, |ui| {
                b.begin_frame();
                clicked = show(ui, b, &feedback, None);
            })
            .drop_without_applying_deltas();
            clicked
        };
        for _ in 0..3 {
            run(&mut b, vec![]);
        }
        // Only the first rows fit.
        assert!(!b.visible.is_empty());
        assert!(b.visible.len() < examples::ALL.len(), "{:?}", b.visible);

        b.tag = Some("lygia");
        run(&mut b, vec![]);
        let lygia: Vec<usize> = (0..examples::ALL.len())
            .filter(|&i| examples::ALL[i].tags.contains(&"lygia"))
            .collect();
        assert_eq!(b.visible, lygia);

        // The first thumbnail sits at the left, below the filter rows.
        let on_first = egui::pos2(60.0, 150.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: on_first,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run(&mut b, vec![egui::Event::PointerMoved(on_first)]);
        let (hovered, pos, start) = b.hovered.expect("hovering the first card");
        assert_eq!(hovered, lygia[0]);
        assert_eq!(pos, start, "a hover starts where the pointer entered");
        assert!(pos.x > 0.0 && pos.y > 0.0 && pos.y < PREVIEW_SIZE.y as f32);
        run(&mut b, vec![button(true)]);
        let clicked = run(&mut b, vec![button(false)]);
        assert_eq!(clicked.map(|e| e.name), Some(examples::ALL[lygia[0]].name));
    }
}
