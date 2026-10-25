//! Drag the picture, not the slider (experimental prototype, native only).
//!
//! In Drag mode (toolbar hand, or Alt+drag on the canvas), grabbing a spot
//! of the image and dragging it changes the constants so that the spot
//! follows the pointer, and the code changes with it.
//!
//! On the press, a background thread renders the shader small
//! ([`EST_WIDTH`] wide) at the frozen time: once as it is, and twice per
//! float constant (component), nudged by ±[`EPSILON`] of its range. Optical
//! flow (iterative Lucas–Kanade with a brightness gain and offset, so color
//! changes do not pass for motion) on a patch around the grab point turns
//! each pair into how far that patch moves per unit of the constant: a 2×N
//! Jacobian. Each frame, the gap between the pointer and where that linear
//! model puts the spot becomes a change of constants (at most
//! [`MAX_MOVE`] at a time): by default of the one constant that best moves
//! the spot along the drag (chosen once the drag has a direction, then
//! kept), or with Shift held a damped-least-squares blend of all of them.
//! While the button is held, whenever the thread is free, it renders the
//! current values, finds the spot again by block matching against the
//! press-time frame, and re-estimates the Jacobian there, so the model's
//! errors are corrected rather than piling up. The values go live through
//! `i_tweaks` (no recompile) and into the source on release, like the
//! Constants window.
//!
//! Integer constants (not differentiable) and colors (they rarely move
//! anything) are left out. Rendering uses [`crate::headless`], so the web
//! build shows the toggle disabled. `--render-drag` runs the same estimate
//! and solve headlessly and measures, by block matching on full-size
//! renders, whether the patch really moved along the drag.

// The flow, the solve and the CLI only run natively (and in tests).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::tweak::{Constant, Hint, Kind, MAX_TWEAKS};

/// Tweak values, slot = index (like [`crate::Tweaks::values`]).
pub type Values = [Vec4; MAX_TWEAKS];

/// Width of the estimation renders, in pixels; the height follows the
/// window's aspect.
const EST_WIDTH: u32 = 192;

/// Radius of the tracked patch, in estimation pixels.
const PATCH_RADIUS: f32 = 10.0;

/// Finite-difference step, as a fraction of a constant's range.
const EPSILON: f32 = 0.015;

/// At most this many constant components take part (two renders each).
const MAX_PARAMS: usize = 48;

/// Largest change of a constant per frame, as a fraction of its range.
const MAX_STEP: f32 = 0.25;

/// Damping of the solve, in estimation pixels: constants that move the spot
/// less than this over their whole range barely count.
const DAMPING_PX: f32 = 4.0;

/// Pointer travel (points) before one-constant mode commits to a constant.
const CHOOSE_AFTER: f32 = 4.0;

/// Most the spot is steered at once, estimation pixels: the linear model
/// only holds nearby, and the tracker looks only so far.
const MAX_MOVE: f32 = 3.0;

/// Distance (estimation pixels) between the spot and the pointer that is
/// close enough.
const MIN_ERROR: f32 = 0.05;

/// One float that can move: a scalar constant or a vector's component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub slot: usize,
    pub component: usize,
    /// The width of its range, which normalizes the solve: a step of 1 is
    /// the whole range.
    pub span: f32,
}

/// The float constants (and components) a drag can move.
pub fn params(constants: &[Constant]) -> Vec<Param> {
    let mut out = Vec::new();
    for (slot, c) in constants.iter().enumerate().take(MAX_TWEAKS) {
        let components = match c.kind {
            Kind::F32 => 1,
            Kind::Vec(n) if !c.is_color() => n as usize,
            _ => continue,
        };
        for component in 0..components {
            let (lo, hi) = c.range(c.value[component]);
            out.push(Param {
                slot,
                component,
                span: (hi - lo).max(1e-6),
            });
        }
    }
    out.truncate(MAX_PARAMS);
    out
}

/// `value` kept within the constant's annotated range, else on the side of
/// zero its source value is on (like Evolve's mutations).
fn bound(c: &Constant, component: usize, value: f32) -> f32 {
    let source = c.value[component];
    match c.hint {
        Hint::Range(lo, hi) => value.clamp(lo, hi),
        _ if source > 0.0 => value.max(0.0),
        _ if source < 0.0 => value.min(0.0),
        _ => value,
    }
}

/// The constant's display name: `NAME` or `NAME.x`.
fn label(constants: &[Constant], p: &Param) -> String {
    let c = &constants[p.slot];
    match c.kind {
        Kind::Vec(_) => format!("{}.{}", c.name, ["x", "y", "z", "w"][p.component]),
        _ => c.name.clone(),
    }
}

// --- images and optical flow --------------------------------------------------

/// A grayscale image, row-major, top row first.
#[derive(Clone)]
pub struct Gray {
    width: usize,
    height: usize,
    px: Vec<f32>,
}

impl Gray {
    /// From tightly packed RGBA32F rows (as [`crate::headless::Renderer`]
    /// reads them back): luma of the clamped color, square-rooted so dark
    /// detail counts roughly as much as it looks, then lightly blurred so
    /// the flow sees a wider basin.
    pub fn from_rgba32f(bytes: &[u8], width: usize, height: usize) -> Self {
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        let px = (0..width * height)
            .map(|i| {
                let [r, g, b] = [f(i * 4), f(i * 4 + 1), f(i * 4 + 2)];
                let luma = 0.299 * r + 0.587 * g + 0.114 * b;
                if luma.is_finite() {
                    luma.clamp(0.0, 1.0).sqrt()
                } else {
                    0.0
                }
            })
            .collect();
        Self { width, height, px }.blurred()
    }

    /// A separable 1-2-1 binomial blur.
    fn blurred(&self) -> Self {
        let (w, h) = (self.width, self.height);
        let get = |px: &[f32], x: isize, y: isize| {
            let x = x.clamp(0, w as isize - 1) as usize;
            let y = y.clamp(0, h as isize - 1) as usize;
            px[y * w + x]
        };
        let mut tmp = vec![0.0; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                tmp[y as usize * w + x as usize] = 0.25 * get(&self.px, x - 1, y)
                    + 0.5 * get(&self.px, x, y)
                    + 0.25 * get(&self.px, x + 1, y);
            }
        }
        let mut px = vec![0.0; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                px[y as usize * w + x as usize] =
                    0.25 * get(&tmp, x, y - 1) + 0.5 * get(&tmp, x, y) + 0.25 * get(&tmp, x, y + 1);
            }
        }
        Self {
            width: w,
            height: h,
            px,
        }
    }

    /// Bilinear sample at pixel coordinates (pixel centres on integers),
    /// clamped to the edge.
    fn at(&self, p: Vec2) -> f32 {
        let x = p.x.clamp(0.0, (self.width - 1) as f32);
        let y = p.y.clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let row = |y: usize| {
            let a = self.px[y * self.width + x0];
            a + (self.px[y * self.width + x1] - a) * fx
        };
        let top = row(y0);
        top + (row(y1) - top) * fy
    }

    fn gradient(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.at(p + Vec2::X) - self.at(p - Vec2::X),
            self.at(p + Vec2::Y) - self.at(p - Vec2::Y),
        ) * 0.5
    }
}

/// How far the patch of `base` around `at` (pixels, y down) moved in
/// `moved`: iterative Lucas–Kanade over a Gaussian-weighted patch, solving
/// for the shift together with a brightness gain and offset. `None` when the
/// patch is flat (nothing to track) or the flow runs away.
pub fn flow(base: &Gray, moved: &Gray, at: Vec2, radius: f32) -> Option<Vec2> {
    let r = radius.ceil() as i32;
    let sigma2 = (radius * 0.5).powi(2);
    let mut samples = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let off = Vec2::new(dx as f32, dy as f32);
            let w = (-off.length_squared() / (2.0 * sigma2)).exp();
            samples.push((at + off, w, base.at(at + off)));
        }
    }
    let total: f32 = samples.iter().map(|s| s.1).sum();
    let mean = samples.iter().map(|s| s.1 * s.2).sum::<f32>() / total;

    // A patch without structure has no position to follow.
    let texture = samples
        .iter()
        .map(|&(p, w, _)| w * base.gradient(p).length_squared())
        .sum::<f32>()
        / total;
    if texture < 1e-6 {
        return None;
    }

    let mut v = Vec2::ZERO;
    for _ in 0..10 {
        // Unknowns: shift (2), gain, offset; moved(p + v + δ) ≈
        // base(p) + gain·(base(p) − mean) + offset.
        let mut a = [[0.0f32; 4]; 4];
        let mut rhs = [0.0f32; 4];
        for &(p, w, b) in &samples {
            let g = moved.gradient(p + v);
            let row = [g.x, g.y, -(b - mean), -1.0];
            let res = b - moved.at(p + v);
            for i in 0..4 {
                for j in 0..4 {
                    a[i][j] += w * row[i] * row[j];
                }
                rhs[i] += w * row[i] * res;
            }
        }
        // Along a straight edge only the normal motion is defined; a little
        // regularization picks the shortest shift instead of failing.
        let reg = 1e-3 * (a[0][0] + a[1][1]) + 1e-9;
        a[0][0] += reg;
        a[1][1] += reg;
        let x = solve4(a, rhs)?;
        let step = Vec2::new(x[0], x[1]);
        v += step;
        if !v.is_finite() || v.length() > radius {
            return None;
        }
        if step.length() < 0.005 {
            break;
        }
    }
    Some(v)
}

/// Solve the 4×4 system `a·x = b` by Gaussian elimination with partial
/// pivoting.
fn solve4(mut a: [[f32; 4]; 4], mut b: [f32; 4]) -> Option<[f32; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let top = a[col];
        for row in col + 1..4 {
            let f = a[row][col] / top[col];
            for (x, t) in a[row].iter_mut().zip(top).skip(col) {
                *x -= f * t;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let rest: f32 = (row + 1..4).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - rest) / a[row][row];
    }
    Some(x)
}

// --- the solve ------------------------------------------------------------------

/// Damping for a Jacobian: [`DAMPING_PX`]², or a tenth of the strongest
/// column if that is more.
fn damping(cols: &[Vec2]) -> f32 {
    let strongest = cols.iter().map(|c| c.length()).fold(0.0, f32::max);
    (DAMPING_PX * DAMPING_PX).max((0.1 * strongest).powi(2))
}

/// The change of every parameter (normalized: 1 = its whole range) that
/// moves the spot by `d` (estimation pixels), by damped least squares:
/// Δ = Jᵀ(JJᵀ + λI)⁻¹·d with J's columns `cols`.
/// Columns under [`MIX_FLOOR`] of the strongest stay out (their motion is
/// mostly estimation noise, and moving them would only rewrite their
/// literals).
pub fn solve_mix(cols: &[Vec2], d: Vec2) -> Vec<f32> {
    let lambda = damping(cols);
    let strongest = cols.iter().map(|c| c.length()).fold(0.0, f32::max);
    let cols: Vec<Vec2> = cols
        .iter()
        .map(|&c| {
            if c.length() < MIX_FLOOR * strongest {
                Vec2::ZERO
            } else {
                c
            }
        })
        .collect();
    let jjt = cols
        .iter()
        .fold(Mat2::from_diagonal(Vec2::splat(lambda)), |m, c| {
            m + Mat2::from_cols(*c * c.x, *c * c.y)
        });
    let y = jjt.inverse() * d;
    cols.iter().map(|c| c.dot(y)).collect()
}

/// See [`solve_mix`].
const MIX_FLOOR: f32 = 0.05;

/// The one parameter that best moves the spot along `d`: the most residual
/// removed, (c·d)² / (|c|² + λ). `None` if none moves it at all.
pub fn choose(cols: &[Vec2], d: Vec2) -> Option<usize> {
    let lambda = damping(cols);
    let score = |c: &Vec2| c.dot(d).powi(2) / (c.length_squared() + lambda);
    (0..cols.len())
        .filter(|&k| cols[k] != Vec2::ZERO)
        .max_by(|&i, &j| score(&cols[i]).total_cmp(&score(&cols[j])))
}

/// The damped step of the single parameter with column `col` toward `d`.
pub fn solve_one(cols: &[Vec2], k: usize, d: Vec2) -> f32 {
    cols[k].dot(d) / (cols[k].length_squared() + damping(cols))
}

/// `values` moved by the normalized steps `steps` (per param), each capped
/// to [`MAX_STEP`] and kept in its constant's bounds.
pub fn apply(
    constants: &[Constant],
    params: &[Param],
    values: &mut Values,
    steps: impl IntoIterator<Item = (usize, f32)>,
) {
    for (k, step) in steps {
        let p = &params[k];
        let v = &mut values[p.slot][p.component];
        let moved = *v + step.clamp(-MAX_STEP, MAX_STEP) * p.span;
        *v = bound(&constants[p.slot], p.component, moved);
    }
}

// --- estimation -------------------------------------------------------------------

/// What one estimate needs: a frame of the shader at fixed inputs.
#[derive(Clone)]
pub struct Job {
    /// The compile it belongs to ([`crate::CurrentShader`]'s generation).
    pub generation: u32,
    pub wgsl: std::sync::Arc<str>,
    pub buffers: u8,
    /// Estimation size, pixels.
    pub size: UVec2,
    pub time: f32,
    /// `i_mouse`, in estimation pixels.
    pub mouse: Vec4,
    pub values: Values,
    pub params: Vec<Param>,
    /// Which press-drag-release it belongs to: the first job of a grab
    /// keeps its frame as the reference the spot is tracked against.
    pub grab: u64,
    /// The press point, estimation pixels, y down.
    pub start: Vec2,
    /// Where the spot should be by now (the model's prediction, or the
    /// pointer before there is a model), same units.
    pub at: Vec2,
}

/// A Jacobian: per param, the patch's motion (estimation pixels, y down)
/// per unit of the normalized param, at the values it was estimated at.
#[derive(Clone)]
pub struct Estimate {
    pub generation: u32,
    pub cols: Vec<Vec2>,
    pub values: Values,
    /// Where the grabbed spot really is at those values (estimation
    /// pixels, y down), found by block matching against the press-time
    /// frame.
    pub spot: Vec2,
    /// Wall time, for the status line.
    pub ms: f32,
}

impl Estimate {
    /// Where the spot is at `values`, by the linear model.
    pub fn predict(&self, params: &[Param], values: &Values) -> Vec2 {
        params
            .iter()
            .zip(&self.cols)
            .fold(self.spot, |at, (p, col)| {
                let by = values[p.slot][p.component] - self.values[p.slot][p.component];
                at + *col * (by / p.span)
            })
    }
}

/// The press-time frame and grab, which later frames track the spot in.
pub struct Reference {
    grab: u64,
    frame: Gray,
}

/// How far from the expected place the tracker looks for the spot,
/// estimation pixels.
const TRACK_SEARCH: i32 = 8;

/// The tracker's pull toward where the model expects the spot (see
/// [`block_match`]).
const TRACK_PRIOR: f32 = 0.01;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::headless::Renderer;
    use std::sync::{Mutex, mpsc};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

    /// Render `values` and read it back as [`Gray`].
    pub fn frame(renderer: &mut Renderer, job: &Job, values: &Values) -> Result<Gray, String> {
        // One frame: buffer shaders start from scratch, which is the
        // best a frozen-time estimate can do for them.
        renderer.render(job.time, 1, values, job.mouse)?;
        let bytes = renderer.read()?;
        Ok(Gray::from_rgba32f(
            &bytes,
            job.size.x as usize,
            job.size.y as usize,
        ))
    }

    /// Where the spot is at `job`'s values, and the Jacobian there: central
    /// differences of the patch flow, two renders per param. The first job
    /// of a grab becomes the `reference`; later ones look for its patch
    /// near where the spot should be by now.
    pub fn estimate(
        renderer: &mut Renderer,
        job: &Job,
        reference: &mut Option<Reference>,
    ) -> Result<Estimate, String> {
        let started = std::time::Instant::now();
        let base = frame(renderer, job, &job.values)?;
        let spot = match reference {
            Some(r) if r.grab == job.grab => {
                let guess = (job.at - job.start).round();
                block_match(
                    &r.frame,
                    &base,
                    job.start,
                    PATCH_RADIUS as i32,
                    (guess.as_ivec2(), TRACK_SEARCH, TRACK_PRIOR),
                )
                .map_or(job.at, |shift| job.start + shift)
            }
            _ => {
                *reference = Some(Reference {
                    grab: job.grab,
                    frame: base.clone(),
                });
                job.start
            }
        };
        let mut cols = Vec::with_capacity(job.params.len());
        for p in &job.params {
            let eps = EPSILON * p.span;
            let mut shifted = |by: f32| -> Result<Option<Vec2>, String> {
                let mut values = job.values;
                values[p.slot][p.component] += by;
                let moved = frame(renderer, job, &values)?;
                Ok(flow(&base, &moved, spot, PATCH_RADIUS))
            };
            let col = match (shifted(eps)?, shifted(-eps)?) {
                (Some(plus), Some(minus)) => (plus - minus) / (2.0 * EPSILON),
                _ => Vec2::ZERO,
            };
            cols.push(col);
        }
        Ok(Estimate {
            generation: job.generation,
            cols,
            values: job.values,
            spot,
            ms: started.elapsed().as_secs_f32() * 1000.0,
        })
    }

    pub fn renderer(job: &Job) -> Result<Renderer, String> {
        pollster::block_on(Renderer::new(
            &job.wgsl,
            job.buffers,
            job.size.x,
            job.size.y,
            FORMAT,
            false,
        ))
    }

    type Results = mpsc::Receiver<Result<Estimate, String>>;

    /// The estimation thread, started on first use. It keeps one renderer
    /// for the latest compile and size, and always works on the newest job.
    #[derive(Default)]
    pub struct Worker {
        channels: Option<(mpsc::Sender<Job>, Mutex<Results>)>,
        /// A job is out and its result not yet back.
        pub busy: bool,
    }

    impl Worker {
        pub fn submit(&mut self, job: Job) {
            let (jobs, _) = self.channels.get_or_insert_with(|| {
                let (job_tx, job_rx) = mpsc::channel::<Job>();
                let (result_tx, result_rx) = mpsc::channel();
                std::thread::Builder::new()
                    .name("drag-estimate".into())
                    .spawn(move || {
                        let mut current: Option<((u32, UVec2), Renderer)> = None;
                        let mut reference = None;
                        while let Ok(mut job) = job_rx.recv() {
                            while let Ok(newer) = job_rx.try_recv() {
                                job = newer;
                            }
                            let key = (job.generation, job.size);
                            if current.as_ref().is_none_or(|(k, _)| *k != key) {
                                current = None;
                                match renderer(&job) {
                                    Ok(r) => current = Some((key, r)),
                                    Err(e) => {
                                        let _ = result_tx.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let (_, r) = current.as_mut().expect("set above");
                            let result = estimate(r, &job, &mut reference);
                            if result_tx.send(result).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("spawn the drag estimation thread");
                (job_tx, Mutex::new(result_rx))
            });
            self.busy = jobs.send(job).is_ok();
        }

        /// The newest result, if one came back.
        pub fn poll(&mut self) -> Option<Result<Estimate, String>> {
            let (_, results) = self.channels.as_ref()?;
            let latest = results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_iter()
                .last();
            if latest.is_some() {
                self.busy = false;
            }
            latest
        }
    }
}

/// Stand-in on the web: nothing to render with, so Drag mode is disabled.
#[cfg(target_arch = "wasm32")]
mod native {
    use super::*;

    #[derive(Default)]
    pub struct Worker {
        pub busy: bool,
    }

    impl Worker {
        pub fn submit(&mut self, _job: Job) {}

        pub fn poll(&mut self) -> Option<Result<Estimate, String>> {
            None
        }
    }
}

/// Whether this build can drag the picture.
pub const AVAILABLE: bool = cfg!(not(target_arch = "wasm32"));

// --- the app side -------------------------------------------------------------------

/// Drag mode's state.
#[derive(Resource)]
pub struct DragPicture {
    /// The toolbar toggle; Alt+drag works without it. Starts on with
    /// `--drag` (native), for scripted checks.
    pub on: bool,
    grab: Option<Grab>,
    /// Grabs so far, to tell their jobs apart.
    grabs: u64,
    worker: native::Worker,
    /// Composed WGSL of the generation it was built for, shared with jobs.
    wgsl: Option<(u32, std::sync::Arc<str>)>,
}

impl Default for DragPicture {
    fn default() -> Self {
        Self {
            on: crate::cli_flag("--drag"),
            grab: None,
            grabs: 0,
            worker: native::Worker::default(),
            wgsl: None,
        }
    }
}

/// One press-drag-release on the canvas.
struct Grab {
    id: u64,
    generation: u32,
    params: Vec<Param>,
    /// The clock speed before the grab froze it.
    speed: f32,
    /// Where the press was, and the pointer (points).
    start: egui::Pos2,
    pointer: egui::Pos2,
    /// Estimation size and the window size it maps from (points).
    size: UVec2,
    viewport: egui::Rect,
    estimate: Option<Estimate>,
    /// One-constant mode's pick, once the drag has a direction.
    chosen: Option<usize>,
    /// The values at the press, for the status line.
    before: Values,
    /// Values the last submitted job was for.
    submitted: Option<(Values, egui::Pos2)>,
    error: Option<String>,
}

impl DragPicture {
    /// Whether a canvas press this frame belongs to Drag mode rather than
    /// `i_mouse`.
    pub fn claims_press(&self, ctx: &egui::Context) -> bool {
        AVAILABLE && (self.on || ctx.input(|i| i.modifiers.alt))
    }
}

/// The toolbar toggle.
pub fn toggle(ui: &mut egui::Ui, drag: &mut DragPicture) {
    let hover = if AVAILABLE {
        "Drag the picture (experimental): grab a spot on the canvas and drag it; \
         the constant that moves it best follows. Shift blends all constants; \
         Alt+drag works without this toggle"
    } else {
        "Drag the picture: native builds only (it renders variants offscreen)"
    };
    ui.add_enabled_ui(AVAILABLE, |ui| {
        ui.toggle_value(&mut drag.on, crate::icon(lucide_icons::Icon::Hand))
            .on_hover_text(hover);
    });
}

/// Everything [`update`] reads or writes.
#[derive(bevy::ecs::system::SystemParam)]
pub struct DragParams<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    drag: ResMut<'w, DragPicture>,
    tweaks: ResMut<'w, crate::Tweaks>,
    state: ResMut<'w, crate::ShaderToyState>,
    current: Res<'w, crate::CurrentShader>,
    evolve: Res<'w, crate::evolve::Evolve>,
    fragment: Single<'w, 's, &'static crate::render::ShaderToyFragment>,
}

/// Start, follow and end grabs; draw the grab marker and status. Runs after
/// [`crate::ui`], which records the canvas.
pub fn update(mut p: DragParams) {
    let Ok(ctx) = p.contexts.ctx_mut() else {
        return;
    };
    let drag = &mut *p.drag;
    let (pointer, pressed, down, mix) = ctx.input(|i| {
        (
            i.pointer.latest_pos(),
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_down(egui::PointerButton::Primary),
            // Shift blends all constants instead of moving the best one.
            i.modifiers.shift,
        )
    });

    if drag.grab.is_none()
        && pressed
        && !p.evolve.on
        && p.current.generation != 0
        && drag.claims_press(ctx)
        && let Some(pos) = pointer.filter(|&pos| crate::pointer_on_canvas(ctx, p.state.canvas, pos))
    {
        let viewport = ctx.viewport_rect();
        let aspect = viewport.height() / viewport.width().max(1.0);
        drag.grabs += 1;
        drag.grab = Some(Grab {
            id: drag.grabs,
            generation: p.current.generation,
            params: params(&p.tweaks.constants),
            speed: p.state.speed,
            start: pos,
            pointer: pos,
            size: UVec2::new(
                EST_WIDTH,
                ((EST_WIDTH as f32 * aspect).round() as u32).max(8),
            ),
            viewport,
            estimate: None,
            chosen: None,
            before: p.tweaks.values,
            submitted: None,
            error: None,
        });
        // Freeze the clock, so animation does not pass for motion.
        p.state.speed = 0.0;
    }
    let Some(grab) = drag.grab.as_mut() else {
        return;
    };

    // Release, or the compile changed under the drag: hand the changed
    // constants to the write-back and restart the clock.
    if !down || p.current.generation != grab.generation {
        for p_ in &grab.params {
            if p.tweaks.values[p_.slot] != grab.before[p_.slot] {
                p.tweaks.dirty |= 1 << p_.slot;
            }
        }
        p.state.speed = grab.speed;
        drag.grab = None;
        return;
    }

    let to_est = |pos: egui::Pos2| {
        let t = (pos - grab.viewport.min) / grab.viewport.size();
        Vec2::new(t.x * grab.size.x as f32, t.y * grab.size.y as f32) - 0.5
    };
    // Outside the window the pointer holds still.
    grab.pointer = pointer.unwrap_or(grab.pointer);
    let pointer = grab.pointer;

    // A fresh Jacobian (for this compile) replaces the old one.
    match drag.worker.poll() {
        Some(Ok(e)) if e.generation == grab.generation => grab.estimate = Some(e),
        Some(Err(e)) => grab.error = Some(e),
        _ => {}
    }

    // Steer the spot toward the pointer: from where the latest estimate
    // found it, the linear model predicts where the current values put
    // it, and the solve covers what is left. A fresh estimate corrects the
    // prediction, so errors of the model do not pile up.
    if let Some(e) = &grab.estimate {
        let crate::Tweaks {
            constants, values, ..
        } = &mut *p.tweaks;
        let d = (to_est(pointer) - e.predict(&grab.params, values)).clamp_length_max(MAX_MOVE);
        if grab.chosen.is_none() && (pointer - grab.start).length() >= CHOOSE_AFTER {
            grab.chosen = choose(&e.cols, to_est(pointer) - to_est(grab.start));
        }
        if d.length() > MIN_ERROR {
            if mix {
                let steps = solve_mix(&e.cols, d);
                apply(
                    constants,
                    &grab.params,
                    values,
                    steps.into_iter().enumerate(),
                );
            } else if let Some(k) = grab.chosen {
                apply(
                    constants,
                    &grab.params,
                    values,
                    [(k, solve_one(&e.cols, k, d))],
                );
            }
        }
    }

    // Keep the Jacobian current: whenever the thread is free and the
    // values or the spot changed, estimate again at the pointer.
    let wanted = (p.tweaks.values, pointer);
    if !drag.worker.busy && !grab.params.is_empty() && grab.submitted != Some(wanted) {
        let wgsl = match &drag.wgsl {
            Some((g, w)) if *g == grab.generation => w.clone(),
            _ => {
                let w: std::sync::Arc<str> = p.current.wgsl.as_str().into();
                drag.wgsl = Some((grab.generation, w.clone()));
                w
            }
        };
        let ppp = ctx.pixels_per_point();
        let scale = grab.size.x as f32 / (grab.viewport.width() * ppp);
        drag.worker.submit(Job {
            generation: grab.generation,
            wgsl,
            buffers: p.fragment.buffers,
            size: grab.size,
            time: p.state.time,
            mouse: p.state.mouse * scale,
            values: p.tweaks.values,
            params: grab.params.clone(),
            grab: grab.id,
            start: to_est(grab.start),
            at: grab.estimate.as_ref().map_or(to_est(pointer), |e| {
                e.predict(&grab.params, &p.tweaks.values)
            }),
        });
        grab.submitted = Some(wanted);
    }

    show_status(ctx, grab, &p.tweaks, pointer, mix);
}

/// The grab marker (press point, line to the pointer) and a status label:
/// which constants moved, and by how much.
fn show_status(
    ctx: &egui::Context,
    grab: &Grab,
    tweaks: &crate::Tweaks,
    pointer: egui::Pos2,
    mix: bool,
) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drag_picture"),
    ));
    let accent = ctx.global_style().visuals.selection.stroke.color;
    let stroke = egui::Stroke::new(1.5, accent);
    let radius = PATCH_RADIUS * grab.viewport.width() / grab.size.x as f32;
    painter.circle_stroke(grab.start, 3.0, stroke);
    painter.line_segment([grab.start, pointer], stroke);
    painter.circle_stroke(pointer, radius, stroke);

    let constants = &tweaks.constants;
    let mut lines: Vec<String> = Vec::new();
    if let Some(e) = &grab.error {
        lines.push(format!("estimate failed: {e}"));
    } else if grab.params.is_empty() {
        lines.push("no float constants to move".into());
    } else if let Some(e) = &grab.estimate {
        if e.cols.iter().all(|c| *c == Vec2::ZERO) {
            lines.push("nothing here moves with the constants".into());
        }
        let mut moved: Vec<(usize, &Param)> = grab
            .params
            .iter()
            .enumerate()
            .filter(|(_, q)| tweaks.values[q.slot][q.component] != grab.before[q.slot][q.component])
            .collect();
        moved.sort_by(|a, b| {
            let change = |q: &Param| {
                ((tweaks.values[q.slot][q.component] - grab.before[q.slot][q.component]) / q.span)
                    .abs()
            };
            change(b.1).total_cmp(&change(a.1))
        });
        for (_, q) in moved.iter().take(4) {
            lines.push(format!(
                "{}  {:.3} → {:.3}",
                label(constants, q),
                grab.before[q.slot][q.component],
                tweaks.values[q.slot][q.component]
            ));
        }
        if moved.is_empty() {
            let what = match grab.chosen {
                Some(k) => label(constants, &grab.params[k]),
                None if mix => "all constants".into(),
                None => "drag to pick a constant".into(),
            };
            lines.push(what);
        }
        lines.push(format!(
            "{} constants · estimate {:.0} ms",
            grab.params.len(),
            e.ms
        ));
    } else {
        lines.push(format!(
            "estimating… ({} renders)",
            1 + 2 * grab.params.len()
        ));
    }
    let text = lines.join("\n");
    let galley = painter.layout_no_wrap(
        text,
        egui::FontId::monospace(12.0),
        ctx.global_style().visuals.text_color(),
    );
    let at = pointer + egui::vec2(radius + 8.0, 8.0);
    let rect = egui::Rect::from_min_size(at, galley.size()).expand(4.0);
    painter.rect_filled(rect, 4.0, ctx.global_style().visuals.window_fill);
    painter.galley(at, galley, egui::Color32::WHITE);
}

// --- `--render-drag` --------------------------------------------------------------

/// `wesl-shadertoy --render-drag <example|file> <x> <y> <dx> <dy>
/// [--time SECS] [--steps N] [--mix] [--out PREFIX]`: grab the 640×360
/// render at (x, y) (pixels, y down), drag it by (dx, dy) in N steps
/// (re-estimating before each, like the app does while the button is held),
/// print which constants moved, and check by block matching on full-size
/// renders that the spot really moved along the drag.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_cli(args: &[String]) {
    let mut iter = args.iter().skip(1);
    let (mut time, mut steps, mut mix, mut out) = (3.0f32, 4u32, false, None);
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--render-drag" => {}
            "--mix" => mix = true,
            "--time" => time = iter.next().and_then(|t| t.parse().ok()).unwrap_or(time),
            "--steps" => steps = iter.next().and_then(|s| s.parse().ok()).unwrap_or(steps),
            "--out" => out = iter.next().cloned(),
            _ => positional.push(arg.clone()),
        }
    }
    let numbers: Vec<f32> = positional
        .iter()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    if positional.len() != 5 || numbers.len() != 4 {
        eprintln!(
            "usage: wesl-shadertoy --render-drag <example-name|file.wesl> <x> <y> <dx> <dy> \
             [--time SECS] [--steps N] [--mix] [--out PREFIX]"
        );
        std::process::exit(2);
    }
    let (name, source) = crate::headless::resolve_source(&positional[0]);
    let compiled = match crate::compile::compile_user_shader(&source) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{name}: compile failed:\n{e}");
            std::process::exit(1);
        }
    };
    match drag_cli(&compiled, &numbers, time, steps.max(1), mix, out.as_deref()) {
        Ok(ok) => std::process::exit(if ok { 0 } else { 1 }),
        Err(e) => {
            eprintln!("{name}: {e}");
            std::process::exit(1);
        }
    }
}

/// The body of [`run_cli`]; returns whether the spot followed the drag.
#[cfg(not(target_arch = "wasm32"))]
fn drag_cli(
    compiled: &crate::compile::CompiledShader,
    numbers: &[f32],
    time: f32,
    steps: u32,
    mix: bool,
    out: Option<&str>,
) -> Result<bool, String> {
    const FULL: UVec2 = UVec2::new(640, 360);
    let size = UVec2::new(EST_WIDTH, EST_WIDTH * FULL.y / FULL.x);
    let scale = size.x as f32 / FULL.x as f32;
    let start = Vec2::new(numbers[0], numbers[1]);
    let drag = Vec2::new(numbers[2], numbers[3]);
    let constants = &compiled.tweaks;
    let params = params(constants);
    let mut values = [Vec4::ZERO; MAX_TWEAKS];
    for (slot, c) in constants.iter().enumerate() {
        values[slot] = Vec4::from_array(c.value);
    }
    let before = values;
    let names: Vec<String> = params.iter().map(|q| label(constants, q)).collect();
    let to_est = |p: Vec2| (p + 0.5) * scale - 0.5;
    println!("{} float constants: {}", params.len(), names.join(", "));

    let mut job = Job {
        generation: 1,
        wgsl: compiled.wgsl.as_str().into(),
        buffers: compiled.buffers,
        size,
        time,
        mouse: Vec4::ZERO,
        values,
        params: params.clone(),
        grab: 1,
        start: to_est(start),
        at: to_est(start),
    };
    let mut renderer = native::renderer(&job)?;
    let mut reference = None;
    let mut chosen = None;
    // Like the app: the pointer moves along the drag in `steps` moves, and
    // after each the spot is found again, the Jacobian re-estimated there,
    // and the constants steered to close the gap; four more rounds settle.
    for i in 0..steps + 4 {
        let pointer = start + drag * ((i + 1) as f32 / steps as f32).min(1.0);
        job.values = values;
        let e = native::estimate(&mut renderer, &job, &mut reference)?;
        if i == 0 {
            println!(
                "Jacobian at ({}, {}), px per whole range ({:.0} ms):",
                start.x, start.y, e.ms
            );
            for (name, c) in names.iter().zip(&e.cols) {
                let c = *c / scale;
                println!("  {name:>20}  ({:7.1}, {:7.1})", c.x, c.y);
            }
        } else {
            let spot = (e.spot + 0.5) / scale - 0.5;
            println!(
                "  round {i}: spot tracked at ({:.1}, {:.1})",
                spot.x, spot.y
            );
        }
        let d = (to_est(pointer) - e.spot).clamp_length_max(MAX_MOVE);
        if mix {
            let steps = solve_mix(&e.cols, d);
            apply(
                constants,
                &params,
                &mut values,
                steps.into_iter().enumerate(),
            );
        } else {
            chosen = chosen.or_else(|| choose(&e.cols, drag));
            if let Some(k) = chosen {
                apply(
                    constants,
                    &params,
                    &mut values,
                    [(k, solve_one(&e.cols, k, d))],
                );
            }
        }
        job.at = e.predict(&params, &values);
    }
    let mut any = false;
    for (q, name) in params.iter().zip(&names) {
        let (a, b) = (before[q.slot][q.component], values[q.slot][q.component]);
        if a != b {
            println!("moved {name}: {a:.4} -> {b:.4}");
            any = true;
        }
    }
    if !any {
        println!("no constant moved");
        return Ok(false);
    }

    // The check: full-size renders before and after, and where the patch
    // went by block matching (independent of the flow estimate).
    let mut full = pollster::block_on(crate::headless::Renderer::new(
        &compiled.wgsl,
        compiled.buffers,
        FULL.x,
        FULL.y,
        wgpu::TextureFormat::Rgba32Float,
        false,
    ))?;
    let mut shot = |values: &Values, png: Option<String>| -> Result<Gray, String> {
        full.render(time, 1, values, Vec4::ZERO)?;
        let bytes = full.read()?;
        if let Some(path) = png {
            crate::headless::write_png(&path, FULL.x, FULL.y, &to_rgba8(&bytes))?;
        }
        Ok(Gray::from_rgba32f(&bytes, FULL.x as usize, FULL.y as usize))
    };
    let a = shot(&before, out.map(|o| format!("{o}-before.png")))?;
    let b = shot(&values, out.map(|o| format!("{o}-after.png")))?;
    // Searched around no motion, with a faint pull toward it, so an
    // ambiguous match (along an edge) never counts in the drag's favour.
    let search = (drag.length() * 1.5 + 12.0).ceil() as i32;
    let Some(measured) = block_match(&a, &b, start, 24, (IVec2::ZERO, search, 1e-4)) else {
        println!(
            "the patch at ({}, {}) is flat; nothing to measure",
            start.x, start.y
        );
        return Ok(false);
    };
    let along = measured.dot(drag) / drag.length_squared().max(1e-6);
    let across = measured.perp_dot(drag) / drag.length().max(1e-6);
    println!(
        "dragged ({:.1}, {:.1}); the spot moved ({:.1}, {:.1}): {:.0}% along the drag, {:.1} px across",
        drag.x,
        drag.y,
        measured.x,
        measured.y,
        along * 100.0,
        across
    );
    Ok(along > 0.5 && across.abs() < 0.5 * drag.length())
}

/// RGBA32F readback to RGBA8 sRGB for a PNG.
#[cfg(not(target_arch = "wasm32"))]
fn to_rgba8(bytes: &[u8]) -> Vec<u8> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let v = f32::from_le_bytes(*b).clamp(0.0, 1.0);
            let encoded = if i % 4 == 3 {
                v
            } else if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round() as u8
        })
        .collect()
}

/// How far the patch of `a` around `at` (radius `r`) moved in `b`: the
/// integer shift within `search` of `guess` with the smallest sum of
/// squared differences, refined by a parabola through its neighbours.
/// `prior` adds that fraction of the patch's contrast energy per squared
/// pixel away from `guess`, so along an edge, where every shift matches
/// about equally, the one nearest the guess wins. `None` for a flat patch.
fn block_match(
    a: &Gray,
    b: &Gray,
    at: Vec2,
    r: i32,
    (guess, search, prior): (IVec2, i32, f32),
) -> Option<Vec2> {
    let patch: Vec<(Vec2, f32)> = (-r..=r)
        .flat_map(|dy| (-r..=r).map(move |dx| Vec2::new(dx as f32, dy as f32)))
        .map(|off| (at + off, a.at(at + off)))
        .collect();
    let mean = patch.iter().map(|p| p.1).sum::<f32>() / patch.len() as f32;
    let energy = patch.iter().map(|p| (p.1 - mean).powi(2)).sum::<f32>();
    if energy < 1e-5 * patch.len() as f32 {
        return None;
    }
    let ssd = |shift: Vec2| -> f32 {
        let ssd: f32 = patch
            .iter()
            .map(|&(p, v)| (b.at(p + shift) - v).powi(2))
            .sum();
        ssd + prior * energy * (shift - guess.as_vec2()).length_squared()
    };
    let mut best = (f32::INFINITY, IVec2::ZERO);
    for dy in -search..=search {
        for dx in -search..=search {
            let shift = guess + IVec2::new(dx, dy);
            let s = ssd(shift.as_vec2());
            if s < best.0 {
                best = (s, shift);
            }
        }
    }
    let centre = best.1.as_vec2();
    let refine = |axis: Vec2| {
        let (m, c, p) = (ssd(centre - axis), best.0, ssd(centre + axis));
        let denom = m - 2.0 * c + p;
        if denom > 1e-9 {
            0.5 * (m - p) / denom
        } else {
            0.0
        }
    };
    Some(centre + Vec2::new(refine(Vec2::X), refine(Vec2::Y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweak::extract;

    /// A soft blob at `centre` on a `w`×`h` image, with brightness `gain`.
    fn blob(w: usize, h: usize, centre: Vec2, gain: f32) -> Gray {
        let px = (0..w * h)
            .map(|i| {
                let p = Vec2::new((i % w) as f32, (i / w) as f32);
                0.1 + gain * (-(p - centre).length_squared() / 30.0).exp()
            })
            .collect();
        Gray {
            width: w,
            height: h,
            px,
        }
    }

    #[test]
    fn flow_finds_subpixel_shifts() {
        let base = blob(64, 48, Vec2::new(30.0, 20.0), 0.8);
        for shift in [
            Vec2::new(0.4, -0.3),
            Vec2::new(2.5, 1.0),
            Vec2::new(-1.2, 3.0),
        ] {
            let moved = blob(64, 48, Vec2::new(30.0, 20.0) + shift, 0.8);
            let v = flow(&base, &moved, Vec2::new(31.0, 21.0), PATCH_RADIUS).unwrap();
            assert!((v - shift).length() < 0.05, "{shift} measured as {v}");
        }
    }

    #[test]
    fn flow_ignores_brightness_and_flat_patches() {
        let base = blob(64, 48, Vec2::new(30.0, 20.0), 0.8);
        let brighter = blob(64, 48, Vec2::new(30.0, 20.0), 0.95);
        let v = flow(&base, &brighter, Vec2::new(33.0, 22.0), PATCH_RADIUS).unwrap();
        assert!(v.length() < 0.05, "a brightness change is not motion: {v}");
        let flat = Gray {
            width: 64,
            height: 48,
            px: vec![0.5; 64 * 48],
        };
        assert!(flow(&flat, &flat, Vec2::new(30.0, 20.0), PATCH_RADIUS).is_none());
    }

    #[test]
    fn solve4_solves() {
        let a = [
            [4.0, 1.0, 0.0, 0.0],
            [1.0, 3.0, 1.0, 0.0],
            [0.0, 1.0, 2.0, 0.5],
            [0.0, 0.0, 0.5, 1.0],
        ];
        let x = [1.0, -2.0, 0.5, 3.0];
        let b = std::array::from_fn(|i| (0..4).map(|j| a[i][j] * x[j]).sum());
        let got = solve4(a, b).unwrap();
        for i in 0..4 {
            assert!((got[i] - x[i]).abs() < 1e-4, "{got:?}");
        }
        assert!(solve4([[0.0; 4]; 4], [1.0; 4]).is_none());
    }

    /// With strong columns the damped solve reproduces the drag, and the
    /// least-norm answer prefers the constant that moves the spot most.
    #[test]
    fn mix_solve_matches_the_drag() {
        let cols = [
            Vec2::new(200.0, 0.0),
            Vec2::new(0.0, 100.0),
            Vec2::new(50.0, 50.0),
        ];
        let d = Vec2::new(4.0, -2.0);
        let steps = solve_mix(&cols, d);
        let achieved: Vec2 = cols.iter().zip(&steps).map(|(c, s)| *c * *s).sum();
        assert!((achieved - d).length() < 0.2, "{achieved}");
        // A drag along x leans on the strong x column.
        let steps = solve_mix(&cols, Vec2::X * 4.0);
        assert!(steps[0].abs() > 5.0 * steps[1].abs(), "{steps:?}");
        // Noise-level columns stay out.
        let steps = solve_mix(&[Vec2::new(200.0, 0.0), Vec2::new(3.0, 1.0)], Vec2::X);
        assert_eq!(steps[1], 0.0);
    }

    #[test]
    fn choose_picks_the_best_aligned_strong_column() {
        let cols = [
            Vec2::new(0.0, 80.0),
            Vec2::new(60.0, 10.0),
            Vec2::new(1.0, 0.0), // aligned but weak
            Vec2::ZERO,
        ];
        assert_eq!(choose(&cols, Vec2::new(10.0, 0.0)), Some(1));
        assert_eq!(choose(&cols, Vec2::new(0.0, -10.0)), Some(0));
        assert_eq!(choose(&[Vec2::ZERO], Vec2::X), None);
        // The step moves along the drag: negative for an opposite drag.
        assert!(solve_one(&cols, 0, Vec2::new(0.0, -10.0)) < 0.0);
        let step = solve_one(&cols, 1, Vec2::new(6.0, 1.0));
        assert!((cols[1] * step - Vec2::new(6.0, 1.0)).length() < 1.0);
    }

    #[test]
    fn params_skip_ints_and_colors_and_steps_respect_bounds() {
        let constants = extract(
            "const SPEED: f32 = 1.0;\n\
             const BAND: f32 = 0.4; // tweak: 0..1\n\
             const N: i32 = 8;\n\
             const TINT: vec3<f32> = vec3<f32>(0.9, 0.5, 0.1);\n\
             const OFFSET: vec2<f32> = vec2<f32>(0.0, -1.0);\n",
        );
        let ps = params(&constants);
        let names: Vec<String> = ps.iter().map(|q| label(&constants, q)).collect();
        assert_eq!(names, ["SPEED", "BAND", "OFFSET.x", "OFFSET.y"]);
        assert_eq!(ps[1].span, 1.0);

        let mut values = [Vec4::ZERO; MAX_TWEAKS];
        for (slot, c) in constants.iter().enumerate() {
            values[slot] = Vec4::from_array(c.value);
        }
        apply(&constants, &ps, &mut values, [(1, 0.2)]);
        assert!((values[1].x - 0.6).abs() < 1e-6);
        apply(&constants, &ps, &mut values, [(1, 10.0)]);
        assert!(
            (values[1].x - 0.6 - MAX_STEP).abs() < 1e-6,
            "steps are capped"
        );
        for _ in 0..10 {
            apply(
                &constants,
                &ps,
                &mut values,
                [(1, 1.0), (0, -1.0), (3, 1.0)],
            );
        }
        assert_eq!(values[1].x, 1.0, "annotated range");
        assert_eq!(values[0].x, 0.0, "SPEED keeps its sign");
        assert_eq!(values[4].y, 0.0, "OFFSET.y keeps its sign");
    }

    #[test]
    fn block_match_finds_the_blob() {
        let a = blob(96, 64, Vec2::new(40.0, 30.0), 0.8);
        let b = blob(96, 64, Vec2::new(46.5, 27.0), 0.8);
        let m = block_match(&a, &b, Vec2::new(40.0, 30.0), 8, (IVec2::ZERO, 10, 0.0)).unwrap();
        assert!((m - Vec2::new(6.5, -3.0)).length() < 0.3, "{m}");
        // Searching around a guess finds it too.
        let m = block_match(
            &a,
            &b,
            Vec2::new(40.0, 30.0),
            8,
            (IVec2::new(5, -2), 3, 0.01),
        )
        .unwrap();
        assert!((m - Vec2::new(6.5, -3.0)).length() < 0.3, "{m}");
    }

    /// Along a straight edge every shift along it matches; the prior picks
    /// the one nearest the guess.
    #[test]
    fn block_match_prior_resolves_edges() {
        let px = (0..96 * 64)
            .map(|i| if i % 96 < 40 { 0.1 } else { 0.9 })
            .collect();
        let edge = Gray {
            width: 96,
            height: 64,
            px,
        };
        let at = Vec2::new(40.0, 30.0);
        let m = block_match(&edge, &edge, at, 6, (IVec2::new(0, 5), 4, 0.01)).unwrap();
        assert!((m - Vec2::new(0.0, 5.0)).length() < 0.3, "{m}");
    }

    /// The linear model moves the spot by each column times the
    /// normalized change.
    #[test]
    fn estimate_predicts_linearly() {
        let params = [
            Param {
                slot: 0,
                component: 0,
                span: 2.0,
            },
            Param {
                slot: 1,
                component: 1,
                span: 0.5,
            },
        ];
        let mut values = [Vec4::ZERO; MAX_TWEAKS];
        values[0].x = 1.0;
        let e = Estimate {
            generation: 1,
            cols: vec![Vec2::new(10.0, 0.0), Vec2::new(0.0, -4.0)],
            values,
            spot: Vec2::new(50.0, 40.0),
            ms: 0.0,
        };
        assert_eq!(e.predict(&params, &values), e.spot);
        values[0].x = 2.0; // +half its range
        values[1].y = 0.25; // +half its range
        assert_eq!(e.predict(&params, &values), Vec2::new(55.0, 38.0));
    }

    /// The estimation thread end to end on the GPU: Metaballs grabbed at a
    /// ball's edge, where ORBIT and STRENGTH both move the spot.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "needs a GPU adapter"]
    fn worker_estimates_on_the_gpu() {
        let example = crate::examples::by_name("Metaballs").unwrap();
        let compiled = crate::compile::compile_user_shader(example.code).unwrap();
        let mut values = [Vec4::ZERO; MAX_TWEAKS];
        for (slot, c) in compiled.tweaks.iter().enumerate() {
            values[slot] = Vec4::from_array(c.value);
        }
        let params = params(&compiled.tweaks);
        let at = Vec2::new(39.0, 24.0);
        let mut worker = native::Worker::default();
        worker.submit(Job {
            generation: 7,
            wgsl: compiled.wgsl.as_str().into(),
            buffers: compiled.buffers,
            size: UVec2::new(EST_WIDTH, 108),
            time: 3.0,
            mouse: Vec4::ZERO,
            values,
            params: params.clone(),
            grab: 1,
            start: at,
            at,
        });
        assert!(worker.busy);
        let started = std::time::Instant::now();
        let e = loop {
            if let Some(result) = worker.poll() {
                break result.unwrap();
            }
            assert!(started.elapsed().as_secs() < 60, "no estimate");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(!worker.busy);
        assert_eq!(e.generation, 7);
        assert_eq!(e.spot, at, "the first job of a grab is its reference");
        let names: Vec<String> = params.iter().map(|q| label(&compiled.tweaks, q)).collect();
        let col = |name: &str| e.cols[names.iter().position(|n| n == name).unwrap()];
        assert!(col("STRENGTH").length() > 10.0, "{:?}", e.cols);
        assert!(col("ORBIT").length() > 10.0, "{:?}", e.cols);
    }
}
