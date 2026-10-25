//! Evolve mode, after Picbreeder: the canvas becomes a 3×3 grid with the
//! current constants in the middle and eight mutations around them.
//! Clicking a variant makes it the new centre and breeds eight more from it;
//! Undo walks back the lineage, Keep writes the centre's values into the
//! source like the Constants window does.
//!
//! No recompile is involved: every variant is the editor's compiled shader
//! with its own `i_tweaks` ([`crate::tweak`]), drawn into an image of its own
//! by the live-thumbnail pass ([`crate::render::previews`]). The variants
//! keep animating with the main clock, and the hovered one sees the pointer
//! as a drag, like the Examples window's thumbnails.
//!
//! A mutation moves each constant by a normally distributed step scaled by
//! the strength slider and the constant's range (`// tweak: a..b`, else the
//! one that follows from its value). Annotated ranges are never left,
//! colors stay colors, unannotated numbers keep their sign, and integers
//! mutate rarely, since they tend to change a shader's structure.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::render::previews::{
    PREVIEW_FORMAT, PreviewFeedback, PreviewJob, PreviewJobs, PreviewKey,
};
use crate::render::{ShaderToyFragment, ShaderToyUniform};
use crate::tweak::{Constant, Hint, Kind, MAX_TWEAKS};

/// Tweak values, slot = index (like [`crate::Tweaks::values`]).
pub type Values = [Vec4; MAX_TWEAKS];

/// Cells in the grid, row by row; [`CENTRE`] holds the parent.
const CELLS: usize = 9;

/// The parent's cell.
const CENTRE: usize = 4;

/// Gap between cells, in points.
const GAP: f32 = 4.0;

/// Height of the controls strip above the grid, in points.
const BAR_HEIGHT: f32 = 32.0;

/// Chance that a variant changes a given float or vector constant.
const FLOAT_RATE: f32 = 0.7;

/// Chance that a variant changes a given integer constant.
const INT_RATE: f32 = 0.12;

/// One standard deviation of a mutation at strength 1, as a fraction of
/// the constant's range.
const SCALE: f32 = 0.5;

/// Evolve mode's state: the lineage, this generation's variants and the
/// images they are drawn into.
#[derive(Resource)]
pub struct Evolve {
    /// The toolbar toggle; Escape, Keep and Discard turn it off. Starts on
    /// with `--evolve` (native), for scripted screenshots.
    pub on: bool,
    /// How far a mutation moves, 0..1 (the slider).
    strength: f32,
    /// The compile the lineage belongs to ([`crate::CurrentShader`]'s
    /// generation); `None` until started. A new compile starts afresh,
    /// since its slots may hold other constants.
    generation: Option<u32>,
    /// Earlier parents, oldest first: what Undo walks back.
    history: Vec<Values>,
    /// This generation's variants; `variants[CENTRE]` is the parent.
    variants: [Values; CELLS],
    rng: Rng,
    /// Per cell: its image and egui's id for it.
    images: Vec<(Handle<Image>, egui::TextureId)>,
    /// Size of each cell in pixels, from the last frame the grid was shown
    /// (zero while it is not).
    size: UVec2,
    /// The hovered cell, with the pointer on it and where the hover began
    /// (pixels, origin bottom-left).
    hovered: Option<(usize, Vec2, Vec2)>,
    /// `hovered` as of the frame before.
    last_hovered: Option<(usize, Vec2, Vec2)>,
}

impl Default for Evolve {
    fn default() -> Self {
        Self {
            on: crate::cli_flag("--evolve"),
            strength: 0.35,
            generation: None,
            history: Vec::new(),
            variants: [[Vec4::ZERO; MAX_TWEAKS]; CELLS],
            rng: Rng::new((crate::now_epoch_secs() * 1.0e6) as u64),
            images: Vec::new(),
            size: UVec2::ZERO,
            hovered: None,
            last_hovered: None,
        }
    }
}

impl Evolve {
    fn parent(&self) -> &Values {
        &self.variants[CENTRE]
    }

    /// Start a fresh lineage from `values`.
    fn start(&mut self, constants: &[Constant], values: Values, generation: u32) {
        self.generation = Some(generation);
        self.history.clear();
        self.variants[CENTRE] = values;
        self.breed(constants);
    }

    /// Fill every cell but the centre with a fresh mutation of the parent.
    fn breed(&mut self, constants: &[Constant]) {
        let parent = *self.parent();
        for i in (0..CELLS).filter(|&i| i != CENTRE) {
            self.variants[i] = mutate(constants, &parent, self.strength, &mut self.rng);
        }
    }

    /// Click on cell `i`: a variant becomes the parent (the centre only
    /// breeds anew).
    fn pick(&mut self, i: usize, constants: &[Constant]) {
        if i != CENTRE {
            self.history.push(*self.parent());
            self.variants[CENTRE] = self.variants[i];
        }
        self.breed(constants);
    }

    /// Back to the previous parent; false at the start of the lineage.
    fn undo(&mut self, constants: &[Constant]) -> bool {
        let Some(parent) = self.history.pop() else {
            return false;
        };
        self.variants[CENTRE] = parent;
        self.breed(constants);
        true
    }

    /// Leave Evolve mode; the next start begins a new lineage.
    fn leave(&mut self) {
        self.on = false;
        self.generation = None;
        self.size = UVec2::ZERO;
    }
}

/// `parent` with some constants mutated (never none, if any can change).
pub fn mutate(constants: &[Constant], parent: &Values, strength: f32, rng: &mut Rng) -> Values {
    let constants = &constants[..constants.len().min(MAX_TWEAKS)];
    let mut child = *parent;
    for (slot, c) in constants.iter().enumerate() {
        let rate = match c.kind {
            Kind::I32 | Kind::U32 => INT_RATE,
            _ => FLOAT_RATE,
        };
        if rng.unit() < rate {
            child[slot] = mutate_one(c, parent[slot], strength, rng);
        }
    }
    // Nothing changed by chance: change one float (or, failing that, any)
    // constant, so no variant repeats its parent.
    let floats: Vec<usize> = (0..constants.len())
        .filter(|&s| !matches!(constants[s].kind, Kind::I32 | Kind::U32))
        .collect();
    let pool: Vec<usize> = if floats.is_empty() {
        (0..constants.len()).collect()
    } else {
        floats
    };
    for _ in 0..8 {
        if child != *parent || pool.is_empty() {
            break;
        }
        let slot = pool[rng.below(pool.len())];
        child[slot] = mutate_one(&constants[slot], parent[slot], strength, rng);
    }
    child
}

/// `value` of constant `c`, moved by one mutation.
fn mutate_one(c: &Constant, mut value: Vec4, strength: f32, rng: &mut Rng) -> Vec4 {
    let step = |rng: &mut Rng, span: f32| rng.normal() * strength * span * SCALE;
    match c.kind {
        Kind::F32 => {
            let (lo, hi) = c.range(c.value[0]);
            value.x = bound(c, c.value[0], value.x + step(rng, hi - lo));
        }
        Kind::I32 | Kind::U32 => {
            // At least one step, so a mutation never rounds away.
            let (lo, hi) = c.range(c.value[0]);
            let mut by = step(rng, hi - lo).round();
            if by == 0.0 {
                by = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
            }
            value.x = bound(c, c.value[0], value.x + by);
            if c.kind == Kind::U32 {
                value.x = value.x.max(0.0);
            }
        }
        // Colors stay colors: rgb within 0..1 (or the source's HDR peak);
        // alpha is left alone.
        Kind::Vec(_) if c.is_color() => {
            for i in 0..3 {
                let top = c.value[i].max(1.0);
                value[i] = (value[i] + step(rng, 1.0)).clamp(0.0, top);
            }
        }
        Kind::Vec(n) => {
            for i in 0..n as usize {
                value[i] = match c.hint {
                    Hint::Range(lo, hi) => (value[i] + step(rng, hi - lo)).clamp(lo, hi),
                    _ => value[i] + step(rng, 2.0 * c.value[i].abs().max(0.1)),
                };
            }
        }
    }
    value
}

/// Keep a scalar within its annotated range, else on the side of zero
/// its source value is on (scales and speeds rarely work negated).
fn bound(c: &Constant, source: f32, v: f32) -> f32 {
    match c.hint {
        Hint::Range(lo, hi) => v.clamp(lo, hi),
        _ if source > 0.0 => v.max(0.0),
        _ if source < 0.0 => v.min(0.0),
        _ => v,
    }
}

/// xorshift64*: mutations need no cryptographic quality, nor a dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in 0..1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in 0..n.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// Roughly standard normal (Irwin–Hall: the sum of four uniforms).
    fn normal(&mut self) -> f32 {
        let sum: f32 = (0..4).map(|_| self.unit()).sum();
        (sum - 2.0) * 3f32.sqrt()
    }
}

/// Allocate the cells' images at the size the grid was laid out with and
/// queue the nine variants as [`PreviewJobs`]. Runs after
/// [`crate::browser::update_previews`], which clears the list.
pub fn update_previews(
    mut evolve: ResMut<Evolve>,
    mut contexts: EguiContexts,
    mut images: ResMut<Assets<Image>>,
    mut jobs: ResMut<PreviewJobs>,
    state: Res<crate::ShaderToyState>,
    fragment: Single<&ShaderToyFragment>,
    time: Res<Time>,
) {
    let e = &mut *evolve;
    let size = e.size;
    if !e.on || size.min_element() == 0 || fragment.generation == 0 {
        return;
    }
    let target = || Image::new_target_texture(size.x, size.y, PREVIEW_FORMAT, None);
    while e.images.len() < CELLS {
        let image = images.add(target());
        let texture = contexts.add_image(bevy_egui::EguiTextureHandle::Strong(image.clone()));
        e.images.push((image, texture));
    }
    for (image, _) in &e.images {
        // Replaced behind the same handle, so egui's id stays valid.
        if images.get(image).is_some_and(|i| i.size() != size) {
            images
                .insert(image, target())
                .expect("evolve image handles stay valid");
        }
    }

    let now = state.time;
    let resolution = Vec4::new(size.x as f32, size.y as f32, 0.0, 0.0);
    let defined = |c: usize| fragment.buffers & (1 << c) != 0;
    for i in 0..CELLS {
        let mouse = match e.hovered {
            Some((h, pos, start)) if h == i => Vec4::new(pos.x, pos.y, start.x, start.y),
            _ => Vec4::ZERO,
        };
        let uniform = ShaderToyUniform {
            time: now,
            time_delta: time.delta_secs().min(0.1) * state.speed,
            resolution: Vec4::new(resolution.x, resolution.y, 1.0, 0.0),
            mouse,
            date: crate::date_uniforms(),
            sample_rate: crate::SAMPLE_RATE_HZ,
            frame_rate: state.fps,
            channel_time: Vec4::from(std::array::from_fn::<f32, 4, _>(|c| {
                if defined(c) { now } else { 0.0 }
            })),
            channel_resolution: std::array::from_fn(|c| {
                if defined(c) { resolution } else { Vec4::ZERO }
            }),
            tweaks: e.variants[i],
            ..default()
        };
        jobs.0.push(PreviewJob {
            card: PreviewKey::Evolve(i),
            image: e.images[i].0.id(),
            size,
            generation: fragment.generation,
            shader: fragment.handle.clone(),
            buffers: fragment.buffers,
            uniform,
        });
    }
}

/// The grid over `canvas` while Evolve mode is on, with its controls on
/// top. `generation` is the latest compile's; Keep writes the centre into
/// `tweaks` and marks it for write-back into the source.
pub fn show(
    ctx: &egui::Context,
    e: &mut Evolve,
    tweaks: &mut crate::Tweaks,
    canvas: egui::Rect,
    generation: u32,
    feedback: &PreviewFeedback,
) {
    e.last_hovered = e.hovered.take();
    if !e.on {
        e.leave();
        return;
    }
    let constants = &tweaks.constants;
    if e.generation != Some(generation) {
        e.start(constants, tweaks.values, generation);
    }

    // Ctrl+Z undoes and Escape leaves, unless a text field has the keys.
    let (undo, escape) = ctx.input(|i| {
        let cmd = i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt;
        (
            cmd && i.key_pressed(egui::Key::Z),
            i.key_pressed(egui::Key::Escape),
        )
    });
    let free = ctx.memory(|m| m.focused().is_none());
    if free && undo {
        e.undo(constants);
    }
    if free && escape {
        e.leave();
        return;
    }

    let mut keep = false;
    egui::Area::new(egui::Id::new("evolve_grid"))
        .order(egui::Order::Background)
        .fixed_pos(canvas.min)
        .constrain(false)
        .show(ctx, |ui| {
            ui.allocate_rect(canvas, egui::Sense::hover());
            ui.painter().rect_filled(canvas, 0.0, egui::Color32::BLACK);
            let bar = egui::Rect::from_min_size(canvas.min, egui::vec2(canvas.width(), BAR_HEIGHT));
            ui.painter().rect_filled(bar, 0.0, ui.visuals().panel_fill);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(bar.shrink2(egui::vec2(8.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| keep = controls(ui, e, constants),
            );
            if !e.on {
                return;
            }
            let grid = egui::Rect::from_min_max(egui::pos2(canvas.min.x, bar.max.y), canvas.max);
            if constants.is_empty() || generation == 0 {
                e.size = UVec2::ZERO;
                let why = if generation == 0 {
                    "Compile a shader first."
                } else {
                    "No tweakable constants: nothing to evolve. Module-level \
                     constants with literal values evolve, e.g. `const SPEED: f32 = 1.0;`"
                };
                ui.painter().text(
                    grid.center(),
                    egui::Align2::CENTER_CENTER,
                    why,
                    egui::FontId::proportional(14.0),
                    ui.visuals().weak_text_color(),
                );
                return;
            }
            cells(ui, e, constants, grid.shrink(GAP), feedback);
        });

    if keep {
        keep_parent(e, tweaks);
    }
}

/// Keep: the parent's values become the live ones, marked dirty so the
/// Constants window's write-back puts them into the source; then leave.
fn keep_parent(e: &mut Evolve, tweaks: &mut crate::Tweaks) {
    let count = tweaks.constants.len();
    let slots = tweaks.values.iter_mut().zip(e.parent()).take(count);
    for (slot, (value, kept)) in slots.enumerate() {
        if value != kept {
            *value = *kept;
            tweaks.dirty |= 1 << slot;
        }
    }
    e.leave();
}

/// The controls strip; returns whether Keep was clicked.
fn controls(ui: &mut egui::Ui, e: &mut Evolve, constants: &[Constant]) -> bool {
    ui.strong("Evolve");
    ui.add(
        egui::Slider::new(&mut e.strength, 0.05..=1.0)
            .text("mutation")
            .max_decimals(2),
    )
    .on_hover_text("How far the variants stray from the centre");
    if ui
        .add_enabled(
            !e.history.is_empty(),
            egui::Button::new(crate::icon(lucide_icons::Icon::Undo2)),
        )
        .on_hover_text("Back to the previous parent (Ctrl+Z)")
        .clicked()
    {
        e.undo(constants);
    }
    if ui
        .button(crate::icon(lucide_icons::Icon::Dices))
        .on_hover_text("Eight new mutations of the centre (or click the centre)")
        .clicked()
    {
        e.breed(constants);
    }
    ui.weak(format!("generation {}", e.history.len()));
    ui.separator();
    let keep = ui
        .button("Keep")
        .on_hover_text("Write the centre's constants into the code and leave")
        .clicked();
    if ui
        .button("Discard")
        .on_hover_text("Leave without changing the code (Escape)")
        .clicked()
    {
        e.leave();
    }
    keep
}

/// The 3×3 grid in `grid`: each cell shows its variant, follows the pointer
/// while hovered and breeds from its variant when clicked.
fn cells(
    ui: &mut egui::Ui,
    e: &mut Evolve,
    constants: &[Constant],
    grid: egui::Rect,
    feedback: &PreviewFeedback,
) {
    let cell = (grid.size() - egui::Vec2::splat(2.0 * GAP)) / 3.0;
    let ppp = ui.ctx().pixels_per_point();
    e.size = UVec2::new(
        (cell.x * ppp).round().max(1.0) as u32,
        (cell.y * ppp).round().max(1.0) as u32,
    );
    let mut picked = None;
    for i in 0..CELLS {
        let (row, col) = ((i / 3) as f32, (i % 3) as f32);
        let min = grid.min + egui::vec2(col * (cell.x + GAP), row * (cell.y + GAP));
        let rect = egui::Rect::from_min_size(min, cell);
        let response = ui.interact(rect, ui.id().with(("evolve_cell", i)), egui::Sense::click());
        let painter = ui.painter();
        match e.images.get(i) {
            Some((_, texture)) if feedback.drawn(PreviewKey::Evolve(i)) => {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(*texture, rect, uv, egui::Color32::WHITE);
            }
            _ => {
                painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            }
        }
        let stroke = if i == CENTRE {
            ui.visuals().selection.stroke
        } else if response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            egui::Stroke::NONE
        };
        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        if let Some(pos) = response.hover_pos() {
            // Cell pixels, origin bottom-left like i_mouse.
            let t = (pos - rect.min) / rect.size();
            let px = Vec2::new(t.x * e.size.x as f32, (1.0 - t.y) * e.size.y as f32);
            let start = match e.last_hovered {
                Some((h, _, start)) if h == i => start,
                _ => px,
            };
            e.hovered = Some((i, px, start));
        }
        if response.clicked() {
            picked = Some(i);
        }
    }
    if let Some(i) = picked {
        e.pick(i, constants);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweak::extract;

    const SRC: &str = "\
const SPEED: f32 = 1.5;
const BAND: f32 = 0.4; // tweak: 0..1
const STEPS: i32 = 8; // tweak: 1..32
const TINT: vec3<f32> = vec3<f32>(0.9, 0.5, 0.1);
const OFFSET: vec2<f32> = vec2<f32>(0.0, -1.0);
";

    fn source_values(constants: &[Constant]) -> Values {
        let mut values = [Vec4::ZERO; MAX_TWEAKS];
        for (slot, c) in constants.iter().enumerate() {
            values[slot] = Vec4::from_array(c.value);
        }
        values
    }

    /// Mutations keep to annotated ranges, colors and signs, integers
    /// change rarely, and no variant repeats its parent.
    #[test]
    fn mutations_respect_hints() {
        let constants = extract(SRC);
        let parent = source_values(&constants);
        let mut rng = Rng::new(7);
        let mut int_changes = 0;
        let runs = 2000;
        for _ in 0..runs {
            let child = mutate(&constants, &parent, 1.0, &mut rng);
            assert_ne!(child, parent);
            assert!(child[0].x >= 0.0, "SPEED keeps its sign: {}", child[0].x);
            assert!((0.0..=1.0).contains(&child[1].x), "BAND: {}", child[1].x);
            assert!((1.0..=32.0).contains(&child[2].x), "STEPS: {}", child[2].x);
            assert_eq!(child[2].x.fract(), 0.0, "STEPS stays whole");
            for v in child[3].truncate().to_array() {
                assert!((0.0..=1.0).contains(&v), "TINT: {v}");
            }
            assert_eq!(child[3].w, 0.0);
            int_changes += usize::from(child[2] != parent[2]);
        }
        let rate = int_changes as f32 / runs as f32;
        assert!(rate < 0.25, "integers mutate rarely: {rate}");
        assert!(rate > 0.0, "but they do mutate");
    }

    #[test]
    fn strength_scales_the_step() {
        let constants = extract("const X: f32 = 1.0;");
        let parent = source_values(&constants);
        let spread = |strength| {
            let mut rng = Rng::new(3);
            (0..500)
                .map(|_| (mutate(&constants, &parent, strength, &mut rng)[0].x - 1.0).abs())
                .sum::<f32>()
        };
        assert!(spread(0.1) * 4.0 < spread(1.0));
    }

    /// Picking breeds from the variant, Undo walks back to each earlier
    /// parent, and the centre only breeds anew.
    #[test]
    fn lineage_picks_and_undoes() {
        let constants = extract(SRC);
        let start = source_values(&constants);
        let mut e = Evolve::default();
        e.start(&constants, start, 1);
        assert_eq!(*e.parent(), start);
        let first = e.variants[0];
        e.pick(0, &constants);
        assert_eq!(*e.parent(), first);
        let second = e.variants[8];
        e.pick(8, &constants);
        assert_eq!(*e.parent(), second);
        let before = e.variants[1];
        e.pick(CENTRE, &constants);
        assert_eq!(*e.parent(), second, "the centre stays");
        assert_ne!(e.variants[1], before, "but breeds anew");
        assert_eq!(e.history.len(), 2);

        assert!(e.undo(&constants));
        assert_eq!(*e.parent(), first);
        assert!(e.undo(&constants));
        assert_eq!(*e.parent(), start);
        assert!(!e.undo(&constants), "nothing before the start");
    }

    /// Clicking a cell breeds from it; Keep hands its values to the
    /// Constants machinery (marked dirty for write-back) and leaves, Escape
    /// just leaves.
    #[test]
    fn grid_clicks_and_keep() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let canvas = screen.shrink2(egui::vec2(0.0, 30.0));
        let feedback = PreviewFeedback::default();
        let constants = extract(SRC);
        let mut tweaks = crate::Tweaks {
            values: source_values(&constants),
            constants,
            ..Default::default()
        };
        let mut e = Evolve {
            on: true,
            ..Default::default()
        };
        let run = |e: &mut Evolve, tweaks: &mut crate::Tweaks, events| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| show(ui.ctx(), e, tweaks, canvas, 1, &feedback))
                .drop_without_applying_deltas();
        };
        let click = |pos| {
            [true, false].map(|pressed| {
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]
            })
        };
        for _ in 0..2 {
            run(&mut e, &mut tweaks, vec![]);
        }
        assert!(e.size.x > 0 && e.size.y > 0);

        // The top-left cell.
        let on_cell = canvas.min + egui::vec2(40.0, BAR_HEIGHT + 40.0);
        run(
            &mut e,
            &mut tweaks,
            vec![egui::Event::PointerMoved(on_cell)],
        );
        assert_eq!(e.hovered.map(|(i, ..)| i), Some(0));
        let chosen = e.variants[0];
        for events in click(on_cell) {
            run(&mut e, &mut tweaks, events);
        }
        assert_eq!(*e.parent(), chosen);
        assert_eq!(e.history.len(), 1);
        assert_eq!(tweaks.dirty, 0, "nothing is written while evolving");

        keep_parent(&mut e, &mut tweaks);
        assert!(!e.on);
        assert_ne!(tweaks.dirty, 0);
        assert_eq!(tweaks.values, chosen);

        // Escape leaves without touching anything.
        let before = tweaks.values;
        e.on = true;
        run(&mut e, &mut tweaks, vec![]);
        assert!(e.history.is_empty(), "a new lineage");
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        run(&mut e, &mut tweaks, vec![escape]);
        assert!(!e.on);
        assert_eq!(tweaks.values, before);
    }
}
