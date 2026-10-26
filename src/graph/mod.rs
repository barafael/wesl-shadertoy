//! The Graph window: the dataflow of one user function as a node graph,
//! read from the naga IR of the latest successful compile.
//!
//! The composed WGSL ([`crate::CurrentShader::wgsl`]) is parsed and
//! validated again whenever the window shows a new compile, function or
//! mode; [`extract`] turns the function into nodes and edges, [`layout`]
//! places them in columns (inputs left, `return` right), and [`show`]
//! paints them on a pannable, zoomable canvas.
//!
//! Named `let`s of `main_image` get live thumbnails: a variant of the
//! shader returns that value as the color right after its declaration
//! (scalars through a diverging palette: warm positive, cool negative),
//! rendered by the example browser's preview machinery
//! ([`crate::render::previews`]) into a small image of its own. Each
//! thumbnail slot keeps one shader asset and one image for the whole
//! session, so recompiles rebuild pipelines in place instead of piling
//! up new ones. Known rough edges: after a recompile a slot shows its old
//! image until the new pipeline is built, a `let` inside a branch shows the
//! normal image wherever the branch is not taken, and a variant the GPU
//! rejects just stops drawing (it passed naga like the original, so this
//! is unlikely).
//!
//! Not done: hovering a node does not highlight its source in the editor
//! (naga spans point into the composed WGSL, not the editor text), user
//! function calls cannot be expanded in place, and control flow shows only
//! as each node's enclosing scope, not as regions.

pub mod extract;
pub mod layout;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::render::ShaderToyUniform;
use crate::render::previews::{
    PREVIEW_FORMAT, PREVIEW_SIZE, PreviewFeedback, PreviewJob, PreviewJobs, PreviewKey,
};
use extract::{Graph, Kind, Mode};
use layout::Layout;

/// Thumbnail slots: at most this many `let`s get one.
const MAX_THUMBS: usize = 8;

/// Thumbnails drawn per frame besides the hovered one, round-robin.
const BUDGET: usize = 3;

/// Size of a thumbnail inside its node, in graph units.
const THUMB: egui::Vec2 = egui::vec2(128.0, 72.0);

/// Monospace advance per character at [`FONT`] size, for sizing boxes
/// before anything is painted (egui's Hack at 12 pt).
const CHAR_WIDTH: f32 = 7.3;
const FONT: f32 = 12.0;
const LINE: f32 = 16.0;
const PAD: f32 = 6.0;

/// Frames a fit is repeated for (see [`GraphView::fit`]).
const FIT_FRAMES: u8 = 4;

/// Fitting the graph to the window never zooms out further than this.
const MIN_FIT_ZOOM: f32 = 0.6;

/// Formula characters shown in a box (the hover shows all of it).
const MAX_LINE: usize = 44;

/// The window's state.
#[derive(Resource)]
pub struct GraphView {
    /// Selected function (readable name); empty picks `main_image`.
    function: String,
    mode: Mode,
    built: Option<Built>,
    pan: egui::Vec2,
    zoom: f32,
    /// Frames left to fit the graph into the canvas: a few, since a
    /// window that just opened is still settling its size.
    fit: u8,
    slots: Vec<Slot>,
    /// Set while the window is shown; [`update_thumbnails`] draws only then.
    shown: bool,
    /// The node under the pointer, as a thumbnail slot.
    hovered_slot: Option<usize>,
    next: usize,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            function: String::new(),
            mode: Mode::Lets,
            built: None,
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            fit: FIT_FRAMES,
            slots: Vec::new(),
            shown: false,
            hovered_slot: None,
            next: 0,
        }
    }
}

/// What the graph was built from, and the result.
struct Built {
    key: (u32, String, Mode),
    /// Buffer passes the compile defines; every variant runs them too.
    buffers: u8,
    functions: Vec<String>,
    result: Result<(Graph, Layout), String>,
    /// Per node: its thumbnail slot.
    thumbs: Vec<Option<usize>>,
    /// Per slot: the variant shader that shows it (`None`: cannot be shown).
    variants: Vec<Option<Variant>>,
}

/// A thumbnail's shader: the compile with `main_image` returning one `let`.
struct Variant {
    wgsl: String,
    /// Of `wgsl`, so a slot can tell cheaply whether it holds this variant.
    hash: u64,
}

impl Variant {
    fn new(wgsl: String) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        wgsl.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            wgsl,
        }
    }
}

/// One thumbnail: image, shader asset, and which variant it holds.
struct Slot {
    image: Handle<Image>,
    texture: egui::TextureId,
    shader: Handle<Shader>,
    /// [`Variant::hash`] of the variant now in `shader`.
    holds: Option<u64>,
    /// Bumped whenever `holds` changes, so the slot's `i_frame` and buffers
    /// restart with the new variant.
    generation: u32,
    buffers: u8,
    last_drawn: f32,
}

/// Parse and validate composed WGSL.
pub fn parse(wgsl: &str) -> Result<(naga::Module, naga::valid::ModuleInfo), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(wgsl))?;
    Ok((module, info))
}

impl GraphView {
    /// Rebuild the graph if the compile, function or mode changed.
    fn refresh(&mut self, wgsl: &str, generation: u32, buffers: u8) {
        let key = (generation, self.function.clone(), self.mode);
        if self.built.as_ref().is_some_and(|b| b.key == key) {
            return;
        }
        self.fit = FIT_FRAMES;
        let mut built = Built {
            key,
            buffers,
            functions: Vec::new(),
            result: Err("Compile a shader to see its graph.".into()),
            thumbs: Vec::new(),
            variants: Vec::new(),
        };
        if generation != 0 {
            match parse(wgsl) {
                Ok((module, info)) => {
                    build(&mut built, &self.function, self.mode, wgsl, &module, &info)
                }
                Err(e) => built.result = Err(e),
            }
        }
        self.built = Some(built);
    }
}

/// Extract and lay out the selected function, and prepare a thumbnail
/// variant for each of its first named `let`s that vary per pixel
/// (`main_image` only).
fn build(
    built: &mut Built,
    function: &str,
    mode: Mode,
    wgsl: &str,
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
) {
    let functions = extract::user_functions(module);
    built.functions = functions.iter().map(|(n, _)| n.clone()).collect();
    let picked = functions
        .iter()
        .find(|(n, _)| n == function)
        .or(functions.first());
    let Some((name, handle)) = picked else {
        built.result = Err("No user functions.".into());
        return;
    };
    let graph = extract::extract(module, info, *handle, mode);
    let sizes: Vec<egui::Vec2> = graph.nodes.iter().map(|n| node_size(n, false)).collect();
    built.thumbs = vec![None; graph.nodes.len()];
    let varies = reachable_from_arguments(&graph);
    if name == "main_image" {
        for (i, node) in graph.nodes.iter().enumerate() {
            if built.variants.len() == MAX_THUMBS {
                break;
            }
            // A let no argument flows into (`i_resolution().xy`, a literal
            // vector) is the same for every pixel: a flat swatch.
            if let Some(expr) = node.expr.filter(|_| varies[i])
                && let Some(variant) = thumbnail_variant(wgsl, module, info, *handle, expr)
            {
                built.thumbs[i] = Some(built.variants.len());
                built.variants.push(Some(Variant::new(variant)));
            }
        }
    }
    let sizes: Vec<egui::Vec2> = sizes
        .iter()
        .zip(&built.thumbs)
        .zip(&graph.nodes)
        .map(|((s, t), n)| if t.is_some() { node_size(n, true) } else { *s })
        .collect();
    let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.from, e.to)).collect();
    let layout = layout::layout(&sizes, &edges);
    built.result = Ok((graph, layout));
}

/// Per node: whether a function argument flows into it.
fn reachable_from_arguments(graph: &Graph) -> Vec<bool> {
    let mut reached: Vec<bool> = graph
        .nodes
        .iter()
        .map(|n| n.kind == Kind::Argument)
        .collect();
    let mut stack: Vec<usize> = (0..reached.len()).filter(|&i| reached[i]).collect();
    while let Some(n) = stack.pop() {
        for e in graph.edges.iter().filter(|e| e.from == n) {
            if !reached[e.to] {
                reached[e.to] = true;
                stack.push(e.to);
            }
        }
    }
    reached
}

/// The composed shader, changed so that `main_image` returns the value of
/// the named `let` `expr` (as a color) right after declaring it. `None`
/// for values with no sensible color (structs, arrays, matrices) or if the
/// rewrite does not validate.
fn thumbnail_variant(
    wgsl: &str,
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    function: naga::Handle<naga::Function>,
    expr: naga::Handle<naga::Expression>,
) -> Option<String> {
    let func = &module.functions[function];
    let name = func.named_expressions.get(&expr)?;
    // The span covers the initializer; the declaration ends at the next `;`
    // (WGSL expressions cannot contain one).
    let end = func.expressions.get_span(expr).to_range()?.end;
    let at = end + wgsl.get(end..)?.find(';')? + 1;
    let color = match info[function][expr].ty.inner_with(&module.types) {
        naga::TypeInner::Scalar(s) => match s.kind {
            naga::ScalarKind::Bool => format!("st_graph_scalar(select(0.0, 1.0, {name}))"),
            naga::ScalarKind::Float if s.width == 4 => format!("st_graph_scalar({name})"),
            _ => format!("st_graph_scalar(f32({name}))"),
        },
        naga::TypeInner::Vector { size, scalar } => {
            let size = *size;
            let n = size as u8;
            let v = match scalar.kind {
                naga::ScalarKind::Float if scalar.width == 4 => name.clone(),
                naga::ScalarKind::Bool => {
                    format!("select(vec{n}<f32>(0.0), vec{n}<f32>(1.0), {name})")
                }
                _ => format!("vec{n}<f32>({name})"),
            };
            match n {
                2 => format!("vec4<f32>({v}, 0.0, 1.0)"),
                3 => format!("vec4<f32>({v}, 1.0)"),
                _ => format!("vec4<f32>(({v}).xyz, 1.0)"),
            }
        }
        _ => return None,
    };
    let variant = format!(
        "{}\n    return {color};{}\n{THUMB_HELPERS}",
        &wgsl[..at],
        &wgsl[at..]
    );
    match parse(&variant) {
        Ok(_) => Some(variant),
        Err(e) => {
            warn!("no thumbnail for `{name}`: {e}");
            None
        }
    }
}

/// Appended to every thumbnail variant: scalars as a diverging palette,
/// orange for positive, blue for negative, brighter with magnitude.
const THUMB_HELPERS: &str = r#"
fn st_graph_scalar(x: f32) -> vec4<f32> {
    let m = 1.0 - exp(-abs(x));
    let hue = select(vec3<f32>(0.25, 0.55, 1.0), vec3<f32>(1.0, 0.6, 0.15), x >= 0.0);
    return vec4<f32>(hue * m, 1.0);
}
"#;

/// Box size of `node` in graph units: title, formula and type lines, plus
/// the thumbnail if it has one.
fn node_size(node: &extract::Node, thumb: bool) -> egui::Vec2 {
    let chars = lines(node)
        .iter()
        .map(|(text, _)| text.chars().count())
        .max()
        .unwrap_or(0);
    let mut size = egui::vec2(
        (chars as f32 * CHAR_WIDTH).max(48.0) + 2.0 * PAD,
        lines(node).len() as f32 * LINE + 2.0 * PAD,
    );
    if thumb {
        size.x = size.x.max(THUMB.x + 2.0 * PAD);
        size.y += THUMB.y + PAD;
    }
    size
}

/// The text lines of a node's box, and whether each is the weak kind.
fn lines(node: &extract::Node) -> Vec<(String, bool)> {
    let mut out = vec![(node.title.clone(), false)];
    if !node.text.is_empty() && node.text != node.title {
        let text = if node.text.chars().count() > MAX_LINE {
            node.text.chars().take(MAX_LINE - 1).collect::<String>() + "…"
        } else {
            node.text.clone()
        };
        out.push((text, true));
    }
    let mut meta = node.ty.clone();
    if !node.scope.is_empty() {
        meta = format!("{meta} · in {}", node.scope);
    }
    out.push((meta, true));
    out
}

/// Accent color per node kind.
fn accent(kind: Kind) -> egui::Color32 {
    let s = crate::theme::SYNTAX;
    match kind {
        Kind::Argument => s.builtin,
        Kind::Uniform => s.keyword,
        Kind::Let => s.ty,
        Kind::Var => s.attribute,
        Kind::Op => s.comment,
        Kind::Call => s.number,
        Kind::Return => crate::theme::OK_COLOR,
    }
}

/// The window's contents.
pub fn show(
    ui: &mut egui::Ui,
    view: &mut GraphView,
    wgsl: &str,
    generation: u32,
    buffers: u8,
    feedback: &PreviewFeedback,
) {
    view.shown = true;
    view.hovered_slot = None;
    view.refresh(wgsl, generation, buffers);

    ui.horizontal(|ui| {
        let functions = view
            .built
            .as_ref()
            .map(|b| b.functions.clone())
            .unwrap_or_default();
        let current = if view.function.is_empty() {
            functions.first().cloned().unwrap_or_default()
        } else {
            view.function.clone()
        };
        egui::ComboBox::from_id_salt("graph_function")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for f in &functions {
                    if ui.selectable_label(*f == current, f).clicked() {
                        view.function = f.clone();
                    }
                }
            });
        ui.selectable_value(&mut view.mode, Mode::Lets, "named lets")
            .on_hover_text("Only let bindings, vars and returns; formulas inline the rest");
        ui.selectable_value(&mut view.mode, Mode::All, "all expressions")
            .on_hover_text("One node per expression (literals and constants inline)");
        if ui.button("fit").clicked() {
            view.fit = FIT_FRAMES;
        }
        if let Some(Ok((graph, _))) = view.built.as_ref().map(|b| &b.result) {
            ui.weak(format!(
                "{} nodes · {} edges",
                graph.nodes.len(),
                graph.edges.len()
            ));
        }
    });
    // A switch takes effect next frame (the combo box borrows the list).
    view.refresh(wgsl, generation, buffers);

    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let Some(built) = &view.built else { return };
    let (graph, layout) = match &built.result {
        Ok(ok) => ok,
        Err(e) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                e.lines().next().unwrap_or(""),
                egui::FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
            return;
        }
    };

    // Pan by dragging, zoom (about the pointer) by scrolling or pinching.
    if view.fit > 0 && rect.is_positive() {
        let margin = 24.0;
        let fit =
            (rect.size() - egui::Vec2::splat(2.0 * margin)) / layout.size.max(egui::vec2(1.0, 1.0));
        // Shrink to fit, but not past legibility: a bigger graph starts at
        // its inputs (left edge) and pans from there.
        view.zoom = fit.min_elem().clamp(MIN_FIT_ZOOM, 1.0);
        view.pan = ((rect.size() - layout.size * view.zoom) / 2.0).max(egui::vec2(margin, margin));
        view.fit -= 1;
    }
    if response.dragged() {
        view.pan += response.drag_delta();
    }
    if let Some(pointer) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (scroll / 400.0).exp();
        if factor != 1.0 {
            let zoom = (view.zoom * factor).clamp(0.05, 4.0);
            let anchor = pointer - rect.min;
            view.pan = anchor - (anchor - view.pan) * (zoom / view.zoom);
            view.zoom = zoom;
        }
    }
    let (zoom, origin) = (view.zoom, rect.min + view.pan);
    let to_screen = |p: egui::Pos2| origin + p.to_vec2() * zoom;

    let painter = ui.painter_at(rect);
    let hovered = response.hover_pos().and_then(|p| {
        layout
            .rects
            .iter()
            .position(|r| egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max)).contains(p))
    });

    // Edges first, under the boxes; a hovered node's edges stand out.
    let weak = ui.visuals().weak_text_color().gamma_multiply(0.6);
    for (edge, route) in graph.edges.iter().zip(&layout.routes) {
        let lit = hovered.is_some_and(|h| h == edge.from || h == edge.to);
        let color = if lit {
            accent(graph.nodes[edge.from].kind)
        } else {
            weak
        };
        let stroke = egui::Stroke::new(if lit { 2.0 } else { 1.2 }, color);
        let points: Vec<egui::Pos2> = route.points.iter().map(|&p| to_screen(p)).collect();
        let curve = curve_through(&points, route.back, zoom);
        if route.back {
            painter.extend(egui::Shape::dashed_line(&curve, stroke, 6.0, 4.0));
        } else {
            painter.add(egui::Shape::line(curve.clone(), stroke));
        }
        if let Some(&tip) = points.last() {
            let s = 5.0 * zoom.max(0.4);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    tip + egui::vec2(-s, -s * 0.6),
                    tip + egui::vec2(-s, s * 0.6),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
    }

    let fill = ui.visuals().extreme_bg_color;
    let text_color = ui.visuals().text_color();
    let weak_text = ui.visuals().weak_text_color();
    for (i, (node, r)) in graph.nodes.iter().zip(&layout.rects).enumerate() {
        let screen = egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max));
        if !screen.intersects(rect) {
            continue;
        }
        let color = accent(node.kind);
        let width = if hovered == Some(i) { 2.5 } else { 1.0 };
        painter.rect(
            screen,
            2.0,
            fill,
            egui::Stroke::new(width, color),
            egui::StrokeKind::Inside,
        );
        // Too small to read: boxes only.
        if zoom < 0.3 {
            continue;
        }
        let mut y = r.min.y + PAD;
        for (k, (line, is_weak)) in lines(node).into_iter().enumerate() {
            let c = if k == 0 {
                color
            } else if is_weak {
                weak_text
            } else {
                text_color
            };
            painter.text(
                to_screen(egui::pos2(r.min.x + PAD, y)),
                egui::Align2::LEFT_TOP,
                line,
                egui::FontId::monospace(FONT * zoom),
                c,
            );
            y += LINE;
        }
        if let Some(slot) = built.thumbs.get(i).copied().flatten() {
            let thumb = egui::Rect::from_min_size(egui::pos2(r.min.x + PAD, y + PAD / 2.0), THUMB);
            let thumb = egui::Rect::from_min_max(to_screen(thumb.min), to_screen(thumb.max));
            match view.slots.get(slot) {
                Some(s) if feedback.drawn(PreviewKey::Graph(slot)) && s.holds.is_some() => {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(s.texture, thumb, uv, egui::Color32::WHITE);
                }
                _ => {
                    painter.rect_filled(thumb, 0.0, ui.visuals().faint_bg_color);
                }
            }
            if hovered == Some(i) {
                view.hovered_slot = Some(slot);
            }
        }
    }

    if let Some(i) = hovered {
        let node = &graph.nodes[i];
        response.on_hover_ui_at_pointer(|ui| {
            ui.set_max_width(420.0);
            ui.strong(&node.title);
            if !node.text.is_empty() {
                ui.monospace(&node.text);
            }
            ui.weak(&node.ty);
            if !node.scope.is_empty() {
                ui.weak(format!("inside {}", node.scope));
            }
        });
    }
}

/// A smooth curve through `points` (screen space): horizontal tangents at
/// every point, so edges leave and enter boxes sideways. A back edge
/// arcs above its ends.
fn curve_through(points: &[egui::Pos2], back: bool, zoom: f32) -> Vec<egui::Pos2> {
    let mut out = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let reach = egui::vec2(((b.x - a.x).abs() / 2.0).max(40.0 * zoom), 0.0);
        let lift = if back {
            egui::vec2(0.0, -50.0 * zoom)
        } else {
            egui::Vec2::ZERO
        };
        let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
            [a, a + reach + lift, b - reach + lift, b],
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::NONE,
        );
        let flat = bezier.flatten(Some(0.5));
        let skip = usize::from(!out.is_empty());
        out.extend(flat.into_iter().skip(skip));
    }
    out
}

/// Allocate thumbnail slots, load changed variants into their shader
/// assets, and queue this frame's thumbnails as [`PreviewJobs`]. Runs
/// after [`crate::browser::update_previews`], which resets the jobs.
#[allow(clippy::too_many_arguments)]
pub fn update_thumbnails(
    mut view: ResMut<GraphView>,
    mut contexts: EguiContexts,
    mut images: ResMut<Assets<Image>>,
    mut shaders: ResMut<Assets<Shader>>,
    mut jobs: ResMut<PreviewJobs>,
    state: Res<crate::ShaderToyState>,
    tweaks: Res<crate::Tweaks>,
    time: Res<Time>,
) {
    let view = &mut *view;
    if !std::mem::take(&mut view.shown) {
        return;
    }
    let Some(built) = &view.built else { return };
    let wanted = built.variants.len();
    while view.slots.len() < wanted {
        let image = images.add(Image::new_target_texture(
            PREVIEW_SIZE.x,
            PREVIEW_SIZE.y,
            PREVIEW_FORMAT,
            None,
        ));
        let texture = contexts.add_image(bevy_egui::EguiTextureHandle::Strong(image.clone()));
        let path = format!("shadertoy://graph/{}", view.slots.len());
        let shader = shaders.add(Shader::from_wgsl(crate::render::placeholder_wgsl(), path));
        view.slots.push(Slot {
            image,
            texture,
            shader,
            holds: None,
            generation: 0,
            buffers: 0,
            last_drawn: 0.0,
        });
    }
    let holds = |slot: &Slot, variant: &Option<Variant>| {
        variant.as_ref().is_some_and(|v| slot.holds == Some(v.hash))
    };
    // Load at most one changed variant per frame: each is a pipeline build.
    if let Some((k, Some(variant))) = built
        .variants
        .iter()
        .enumerate()
        .find(|(k, v)| v.is_some() && !holds(&view.slots[*k], v))
    {
        let slot = &mut view.slots[k];
        let path = format!("shadertoy://graph/{k}");
        shaders
            .insert(&slot.shader, Shader::from_wgsl(variant.wgsl.clone(), path))
            .expect("slot handles stay valid");
        slot.buffers = built.buffers;
        slot.holds = Some(variant.hash);
        slot.generation += 1;
    }

    let ready: Vec<usize> = (0..wanted)
        .filter(|&k| holds(&view.slots[k], &built.variants[k]))
        .collect();
    if ready.is_empty() {
        return;
    }
    let mut picked: Vec<usize> = view
        .hovered_slot
        .filter(|k| ready.contains(k))
        .into_iter()
        .collect();
    for k in 0..BUDGET.min(ready.len()) {
        let slot = ready[(view.next + k) % ready.len()];
        if !picked.contains(&slot) {
            picked.push(slot);
        }
    }
    view.next = (view.next + BUDGET) % ready.len();

    let now = time.elapsed_secs();
    let size = Vec4::new(PREVIEW_SIZE.x as f32, PREVIEW_SIZE.y as f32, 0.0, 0.0);
    for k in picked {
        let slot = &mut view.slots[k];
        let defined = |c: usize| slot.buffers & (1 << c) != 0;
        let uniform = ShaderToyUniform {
            time: state.time,
            time_delta: (now - slot.last_drawn).clamp(0.0, 0.1),
            resolution: Vec4::new(size.x, size.y, 1.0, 0.0),
            date: crate::date_uniforms(),
            sample_rate: crate::SAMPLE_RATE_HZ,
            frame_rate: 1.0 / time.delta_secs().max(1e-3),
            channel_time: Vec4::from(std::array::from_fn::<f32, 4, _>(|c| {
                if defined(c) { state.time } else { 0.0 }
            })),
            channel_resolution: std::array::from_fn(|c| if defined(c) { size } else { Vec4::ZERO }),
            tweaks: tweaks.values,
            ..default()
        };
        slot.last_drawn = now;
        jobs.0.push(PreviewJob {
            card: PreviewKey::Graph(k),
            image: slot.image.id(),
            size: PREVIEW_SIZE,
            generation: slot.generation,
            shader: slot.shader.clone(),
            buffers: slot.buffers,
            uniform,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window keeps the size it was given: the canvas fills it and
    /// does not push it wider frame by frame.
    #[test]
    fn window_does_not_grow() {
        let example = crate::examples::by_name("Seascape").expect("Seascape");
        let wgsl = crate::compile::compile_user_shader(example.code)
            .expect("compiles")
            .wgsl;
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
        let feedback = PreviewFeedback::default();
        let mut view = GraphView {
            function: "getPixel".into(),
            ..default()
        };
        let mut sizes = Vec::new();
        for _ in 0..12 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let shown = egui::Window::new("Graph")
                    .default_pos([100.0, 60.0])
                    .default_size([760.0, 520.0])
                    .show(ui.ctx(), |ui| show(ui, &mut view, &wgsl, 1, 0, &feedback));
                sizes.push(shown.unwrap().response.rect.size());
            })
            .drop_without_applying_deltas();
        }
        assert!(sizes[11].x < 800.0, "{sizes:?}");
        assert_eq!(sizes[11], sizes[6], "{sizes:?}");
    }

    #[test]
    fn thumbnails_for_the_lets_of_main_image() {
        let example = crate::examples::by_name("Plasma").expect("Plasma");
        let wgsl = crate::compile::compile_user_shader(example.code)
            .expect("compiles")
            .wgsl;
        let mut view = GraphView::default();
        view.refresh(&wgsl, 1, 0);
        let built = view.built.as_ref().unwrap();
        let (graph, _) = built.result.as_ref().expect("graph");
        let with_thumbs: Vec<&str> = graph
            .nodes
            .iter()
            .zip(&built.thumbs)
            .filter(|(_, t)| t.is_some())
            .map(|(n, _)| n.title.as_str())
            .collect();
        // `res` and `t` are the same for every pixel.
        assert_eq!(with_thumbs, ["uv", "col"]);
        let col = &built.variants[1].as_ref().unwrap().wgsl;
        assert!(col.contains("return vec4<f32>(col, 1.0);"), "{col}");

        // Also when the environment reads are nodes of their own.
        let mut all = GraphView {
            mode: Mode::All,
            ..default()
        };
        all.refresh(&wgsl, 1, 0);
        let built = all.built.as_ref().unwrap();
        let (graph, _) = built.result.as_ref().expect("graph");
        let thumbs = built.thumbs.iter().flatten().count();
        assert_eq!(thumbs, 2, "{:?}", graph.nodes);
    }

    /// Every example's `main_image` gets a graph, and every thumbnail
    /// variant that is offered validates (checked inside).
    #[test]
    fn every_example_builds_a_graph() {
        for example in crate::examples::ALL {
            let wgsl = crate::compile::compile_user_shader(example.code)
                .expect("compiles")
                .wgsl;
            for mode in [Mode::Lets, Mode::All] {
                let mut view = GraphView { mode, ..default() };
                view.refresh(&wgsl, 1, 0);
                let built = view.built.as_ref().unwrap();
                assert!(built.result.is_ok(), "{}", example.name);
                assert!(built.variants.len() <= MAX_THUMBS);
            }
        }
    }
}
