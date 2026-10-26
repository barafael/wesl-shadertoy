//! The Examples window: every built-in example as a card with a live
//! thumbnail, filtered by a search box and tags; clicking a card opens it.
//! The tutorial comes first, then the other beginner examples (see
//! [`examples::rank`]).
//!
//! The thumbnails are real renders ([`crate::render::previews`]) of the
//! examples' WGSL, which `build.rs` compiles ([`COMPILED`]), so opening the
//! window compiles nothing on the CPU. The first time a card scrolls into
//! view its WGSL becomes a shader asset of its own that the render world
//! builds pipelines for. Each frame [`update_previews`] then asks for the
//! hovered card plus a few of the visible ones in turn, so a full window
//! costs a handful of small draws per frame. Hovering a card drives its
//! `i_mouse` as a drag, so interactive examples react in place.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::examples::{self, Example};
use crate::render::ShaderToyUniform;
use crate::render::previews::{
    PREVIEW_FORMAT, PREVIEW_SIZE, PreviewFeedback, PreviewJob, PreviewJobs, PreviewKey,
};
use crate::tweak::MAX_TWEAKS;

/// Narrowest thumbnail on screen, in points; cards grow to fill the row
/// (the image itself is always [`PREVIEW_SIZE`]).
const MIN_THUMB_WIDTH: f32 = 200.0;

/// Gap between cards, in points.
const GAP: f32 = 12.0;

/// Visible cards drawn per frame besides the hovered one, round-robin.
const BUDGET: usize = 4;

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
    shader: Option<Handle<Shader>>,
    /// `i_time` when it was last drawn, for its `i_time_delta`.
    last_drawn: f32,
}

/// An example as `build.rs` compiled it.
struct CompiledExample {
    wgsl: &'static str,
    buffers: u8,
    /// Its live constants' default values, slot = index.
    tweaks: &'static [[f32; 4]],
}

// `COMPILED`, one entry per example in `examples::ALL` order.
include!(concat!(env!("OUT_DIR"), "/compiled_examples.rs"));

impl Browser {
    /// Call every frame before [`show`]: a closed window shows no cards.
    pub fn begin_frame(&mut self) {
        self.visible.clear();
        self.last_hovered = self.hovered.take();
    }
}

/// Allocate images and shader assets for the cards the window showed last
/// frame, and pick this frame's [`PreviewJobs`].
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
        card.shader.get_or_insert_with(|| {
            let path = format!("shadertoy://example/{i}");
            shaders.add(Shader::from_wgsl(COMPILED[i].wgsl, path))
        });
    }

    // The hovered card every frame, then the next few visible ones.
    let drawable = |b: &Browser, i: usize| b.cards[i].shader.is_some();
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
        let (Some((image, _)), Some(shader)) = (&card.image, &card.shader) else {
            continue;
        };
        let compiled = &COMPILED[i];
        let mut tweaks = [Vec4::ZERO; MAX_TWEAKS];
        for (slot, value) in compiled.tweaks.iter().enumerate() {
            tweaks[slot] = Vec4::from_array(*value);
        }
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
            tweaks,
            ..default()
        };
        card.last_drawn = now;
        jobs.0.push(PreviewJob {
            card: PreviewKey::Example(i),
            image: image.id(),
            size: PREVIEW_SIZE,
            generation: 0,
            shader: shader.clone(),
            buffers: compiled.buffers,
            uniform,
        });
    }
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
                || [e.name, e.summary, e.author, e.teaches]
                    .iter()
                    .chain(e.tags)
                    .any(|s| s.to_lowercase().contains(&query)))
    };
    let mut shown: Vec<usize> = (0..examples::ALL.len())
        .filter(|&i| matches(&examples::ALL[i]))
        .collect();
    // Newcomers first; a stable sort keeps the rest by name.
    shown.sort_by_key(|&i| examples::rank(&examples::ALL[i]));

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

/// One card: thumbnail, name, summary and what it teaches. Returns whether it was clicked.
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
            if !example.teaches.is_empty() {
                ui.add(egui::Label::new(teaches(example).small()).truncate());
            }
            response
        })
        .inner;
    response.on_hover_ui(|ui| details(ui, example)).clicked()
}

fn thumbnail(ui: &egui::Ui, b: &Browser, i: usize, rect: egui::Rect, feedback: &PreviewFeedback) {
    let painter = ui.painter();
    let card = &b.cards[i];
    match &card.image {
        Some((_, texture)) if feedback.drawn(PreviewKey::Example(i)) => {
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
    if !e.teaches.is_empty() {
        ui.label(teaches(e));
    }
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

/// "teaches …", in the color that sets it apart from the summary.
fn teaches(e: &Example) -> egui::RichText {
    egui::RichText::new(format!("teaches {}", e.teaches)).color(crate::theme::TEACHES_COLOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build-time compiles line up with the examples and match what
    /// compiling them now gives. wesl orders the declarations it emits
    /// differently from one process to the next (its maps are seeded per
    /// process), so the WGSL is compared as a set of lines.
    #[test]
    fn compiled_examples_match_the_examples() {
        assert_eq!(COMPILED.len(), examples::ALL.len());
        for (example, compiled) in examples::ALL.iter().zip(COMPILED) {
            let now = crate::compile::compile_user_shader(example.code).expect("compiles");
            let lines = |wgsl: &str| {
                let mut lines: Vec<String> = wgsl.lines().map(str::to_owned).collect();
                lines.sort();
                lines
            };
            assert_eq!(lines(compiled.wgsl), lines(&now.wgsl), "{}", example.name);
            assert_eq!(compiled.buffers, now.buffers, "{}", example.name);
            let values: Vec<[f32; 4]> = now.tweaks.iter().map(|c| c.value).collect();
            assert_eq!(compiled.tweaks, values, "{}", example.name);
        }
    }

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
        let mut lygia: Vec<usize> = (0..examples::ALL.len())
            .filter(|&i| examples::ALL[i].tags.contains(&"lygia"))
            .collect();
        lygia.sort_by_key(|&i| examples::rank(&examples::ALL[i]));
        // The first rows of them fit.
        assert!(!b.visible.is_empty());
        assert_eq!(b.visible, lygia[..b.visible.len()]);
        assert_eq!(examples::ALL[lygia[0]].name, "LYGIA 101", "tutorial first");

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
