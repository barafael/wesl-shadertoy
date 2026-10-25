//! wesl-shadertoy — a classic shadertoy-style live shader playground for
//! WESL, built on Bevy 0.20 + egui, running natively and in the browser.
//!
//! Architecture in one paragraph: the editor text is compiled at runtime by
//! [`compile`] into a WGSL fragment module (WESL linker + naga validation),
//! wrapped by the hidden `package::main` entry module. The compiled shader is
//! swapped into a fullscreen post-process pass via the vendored-and-modified
//! Bevy `fullscreen_material` plugin ([`render`]), which rebuilds its pipeline
//! whenever the shader handle changes. Per-frame uniforms (iTime, iResolution,
//! iMouse, iDate, iFrame) ride along as a dynamic uniform buffer component.

mod analyzer;
mod browser;
mod compile;
mod debugger;
mod editor;
mod evolve;
mod examples;
#[cfg(not(target_arch = "wasm32"))]
mod headless;
mod highlight;
mod persist;
mod render;
mod theme;
mod tweak;
mod ui_texture;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
// std's Instant panics on wasm32-unknown-unknown; Bevy's platform module is
// web-time under the hood on the web, std elsewhere.
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use bevy::render::error_handler::{ErrorType, RenderError, RenderErrorHandler, RenderErrorPolicy};
use render::active::{ActiveShader, Playback, RejectedGeneration, ShaderFeedback};
use render::{ShaderToyFragment, ShaderToyRenderPlugin, ShaderToyUniform};

/// Strongest frosted-glass blur the settings allow (sigma, in points):
/// beyond it the shader washes out into one flat colour behind the UI.
const MAX_BLUR: f32 = 24.0;

/// Value reported by `i_sample_rate()`. shadertoy reports the audio sample
/// rate; there is no audio here, so we report the conventional 44.1 kHz.
const SAMPLE_RATE_HZ: f32 = 44100.0;

fn main() {
    // Native-only offscreen smoke render; never touches the display server.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--render") {
            headless::run_render_cli(&args);
            return;
        }
    }
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "wesl-shadertoy".into(),
                        present_mode: PresentMode::AutoVsync,
                        // On web the CSS owns the canvas box (see index.html);
                        // let Bevy match the render buffer to it.
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                })
                .set(bevy::log::LogPlugin {
                    level: bevy::log::Level::INFO,
                    filter: "warn,bevy=info,wesl_shadertoy=trace".into(),
                    ..default()
                }),
        )
        .init_resource::<ShaderDoc>()
        .init_resource::<ShaderToyState>()
        .init_resource::<CurrentShader>()
        .init_resource::<CompileStatus>()
        .init_resource::<Tweaks>()
        .init_resource::<RunRequest>()
        .init_resource::<UiWindows>()
        .init_resource::<editor::HighlightCache>()
        .init_resource::<analyzer::Analyzer>()
        .init_resource::<browser::Browser>()
        .init_resource::<evolve::Evolve>()
        .add_plugins(EguiPlugin::default())
        .add_plugins(ShaderToyRenderPlugin)
        .add_plugins(ui_texture::UiTexturePlugin)
        .insert_resource(RenderErrorHandler(on_render_error))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                install_fonts,
                (update_uniforms, compile_and_apply, poll_render_feedback).chain(),
                run_analyzer,
                autosave,
                browser::update_previews,
                evolve::update_previews.after(browser::update_previews),
            ),
        )
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
}

/// The shader source being edited: the app's single source of truth.
#[derive(Resource, Default)]
struct ShaderDoc {
    text: String,
}

/// Playback + input state for the shadertoy clock and iMouse.
#[derive(Resource)]
struct ShaderToyState {
    time: f32,
    /// Clock speed from the toolbar slider; 0 pauses.
    speed: f32,
    mouse: Vec4,
    mouse_held: bool,
    fps: f32,
    /// In-shader debugger overlay on/off (toolbar bug icon).
    debugger: bool,
    /// The example last loaded into the editor (`None` for files and
    /// restored documents); the Examples window marks its card.
    example: Option<&'static str>,
    /// The viewport area the panels leave free (egui points), recorded by
    /// [`ui`] each frame; iMouse only counts presses there.
    canvas: egui::Rect,
}

impl Default for ShaderToyState {
    fn default() -> Self {
        Self {
            time: 0.0,
            speed: 1.0,
            mouse: Vec4::ZERO,
            mouse_held: false,
            fps: 0.0,
            debugger: false,
            example: None,
            canvas: egui::Rect::NOTHING,
        }
    }
}

/// The shader handles the app renders with, and the latest compile.
///
/// Compiles alternate between two slots, replacing the asset of whichever
/// slot is not on screen: Bevy then rebuilds that slot's pipelines in place
/// while the other keeps drawing, and the number of shaders and pipelines
/// stays fixed however often the user compiles (Bevy's pipeline cache never
/// frees a pipeline, and a removed shader leaves its pipelines retrying
/// every frame).
#[derive(Resource, Default)]
struct CurrentShader {
    /// Black stand-in until the first compile, and the fallback after a GPU
    /// rejection with nothing better to show.
    placeholder: Handle<Shader>,
    slots: [Handle<Shader>; 2],
    /// Generation of the latest successful compile (0: none yet).
    generation: u32,
    /// Its composed WGSL, for the WGSL window.
    wgsl: String,
}

impl CurrentShader {
    fn placeholder_fragment(&self) -> ShaderToyFragment {
        ShaderToyFragment {
            handle: self.placeholder.clone(),
            ..default()
        }
    }
}

/// Result of the most recent compile attempt.
#[derive(Resource, Default)]
struct CompileStatus {
    error: Option<String>,
    last_ms: Option<f32>,
    /// Transient "saved to …" / "opened …" message: `(text, ok)`.
    note: Option<(String, bool)>,
}

/// The on-screen shader's live-tweakable constants ([`tweak`]) and their
/// current values, edited in the Constants window.
#[derive(Resource)]
struct Tweaks {
    /// From the latest successful compile; slot = index.
    constants: Vec<tweak::Constant>,
    /// Current value per slot, uploaded every frame as `i_tweaks`.
    values: [Vec4; tweak::MAX_TWEAKS],
    /// Literal constants that have to stay constants (see
    /// [`compile::CompiledShader::fixed`]).
    fixed: Vec<String>,
    /// Bit per slot changed since its value was last written into the source.
    dirty: u32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Self {
            constants: Vec::new(),
            values: [Vec4::ZERO; tweak::MAX_TWEAKS],
            fixed: Vec::new(),
            dirty: 0,
        }
    }
}

/// Set by the Run button / Ctrl+Enter, consumed by [`compile_and_apply`].
#[derive(Resource, Default)]
struct RunRequest(bool);

#[derive(Resource)]
struct UiWindows {
    examples: bool,
    /// An example clicked while the editor holds other work: asks first.
    confirm_example: Option<&'static examples::Example>,
    editor: bool,
    wgsl: bool,
    constants: bool,
    settings: bool,
    about: bool,
    errors_expanded: bool,
}

impl Default for UiWindows {
    fn default() -> Self {
        Self {
            examples: false,
            confirm_example: None,
            editor: true,
            wgsl: false,
            constants: false,
            settings: false,
            about: false,
            errors_expanded: true,
        }
    }
}

/// User preferences, shown in the settings window. Persisted to
/// `localStorage` on the web; in-memory only in native builds.
#[derive(Resource)]
struct Settings {
    random_example: bool,
    /// Live diagnostics from wgsl-analyzer while editing.
    analyzer: bool,
    /// Frosted glass: how blurred the shader shows through windows and
    /// panels, as the blur's sigma in points; 0 is off (opaque UI).
    blur: f32,
}

impl Settings {
    /// Used when nothing is saved yet: random example, live diagnostics and
    /// frosted glass on.
    fn fallback() -> Self {
        Self {
            random_example: true,
            analyzer: true,
            blur: 12.0,
        }
    }

    fn encode(&self) -> String {
        format!(
            "random={};analyzer={};blur_px={}",
            self.random_example as u8, self.analyzer as u8, self.blur
        )
    }

    fn decode(s: &str) -> Self {
        let mut out = Self::fallback();
        for part in s.split(';') {
            match part.split_once('=') {
                Some(("random", r)) => out.random_example = r == "1",
                Some(("analyzer", a)) => out.analyzer = a == "1",
                Some(("blur_px", b)) => {
                    if let Ok(b) = b.parse::<f32>() {
                        out.blur = b.clamp(0.0, MAX_BLUR);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// The app-state resources [`setup`] seeds, bundled into one parameter.
#[derive(SystemParam)]
struct StartupParams<'w> {
    doc: ResMut<'w, ShaderDoc>,
    current: ResMut<'w, CurrentShader>,
    run: ResMut<'w, RunRequest>,
    state: ResMut<'w, ShaderToyState>,
    wins: ResMut<'w, UiWindows>,
}

fn setup(mut commands: Commands, mut shaders: ResMut<Assets<Shader>>, mut app: StartupParams) {
    let settings = persist::load_settings()
        .map(|s| Settings::decode(&s))
        .unwrap_or_else(Settings::fallback);
    let random = settings.random_example;
    commands.insert_resource(settings);

    // `--shader <file>` (native) wins over everything: it is how scripted
    // live tests inject a document. Otherwise restore a saved document
    // (browser localStorage) or start from an example — a random one when
    // the fresh-start setting is on.
    app.state.debugger = cli_flag("--debugger");
    app.wins.examples = cli_flag("--examples");
    if let Some(text) = cli_shader() {
        app.doc.text = text;
    } else {
        match persist::load_doc() {
            Some(saved) => {
                app.doc.text = saved;
            }
            None => {
                let example = cli_example()
                    .or(random.then(random_example))
                    .unwrap_or_else(examples::default);
                app.doc.text = example.code.to_string();
                app.state.example = Some(example.name);
            }
        }
    }

    // A black placeholder so the pipeline has *something* until the first
    // successful compile lands, and the two compile slots (see
    // `CurrentShader`), seeded with the same placeholder source.
    let placeholder = |path: &str| Shader::from_wgsl(render::placeholder_wgsl(), path.to_string());
    app.current.placeholder = shaders.add(placeholder("shadertoy://placeholder"));
    app.current.slots = [
        shaders.add(placeholder("shadertoy://user/0")),
        shaders.add(placeholder("shadertoy://user/1")),
    ];

    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: bevy::camera::ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // Pass the shader's colors through untouched, like shadertoy.
        Tonemapping::None,
        bevy::core_pipeline::tonemapping::DebandDither::Disabled,
        Msaa::Off,
        ShaderToyUniform::default(),
        app.current.placeholder_fragment(),
    ));

    // Compile whatever is in the editor on the first frame.
    app.run.0 = true;
}

/// Embed the Lucide icon font (bundled by the `lucide-icons` crate) as the
/// last fallback of the proportional family. The toolbar and status bar use
/// Lucide's private-use-area codepoints, which no system font carries, so
/// this is what makes the icons render identically on every platform.
/// Runs in Update (once) because bevy_egui only creates the primary context
/// after it first sees a camera, which is too late for a Startup system.
fn install_fonts(mut contexts: EguiContexts, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    ctx.set_fonts(lucide_fonts());
    *done = true;
}

/// egui's default fonts plus the Lucide icon font as the final proportional
/// fallback (the font behind [`icon`]).
fn lucide_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "lucide".to_owned(),
        egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).into(),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("lucide".to_owned());
    fonts
}

/// Everything [`update_uniforms`] reads or writes, bundled into one parameter.
#[derive(SystemParam)]
struct UniformParams<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    state: ResMut<'w, ShaderToyState>,
    time: Res<'w, Time>,
    windows: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    uniforms: Single<'w, 's, &'static mut ShaderToyUniform>,
    playback: ResMut<'w, Playback>,
    debugger_on: ResMut<'w, debugger::DebuggerOn>,
    tweaks: Res<'w, Tweaks>,
}

/// Advance the shadertoy clock, count frames, track iMouse and viewport size.
fn update_uniforms(mut p: UniformParams) {
    // Clamp so a backgrounded tab does not teleport the clock on resume.
    let dt = p.time.delta_secs().min(0.1);
    let step = dt * p.state.speed;
    p.state.time += step;
    let paused = p.state.speed == 0.0;
    if p.playback.paused != paused {
        p.playback.paused = paused;
    }
    if dt > 0.0 {
        p.state.fps = if p.state.fps == 0.0 {
            1.0 / dt
        } else {
            p.state.fps * 0.95 + (1.0 / dt) * 0.05
        };
    }

    let resolution = Vec4::new(
        p.windows.resolution.physical_width() as f32,
        p.windows.resolution.physical_height() as f32,
        1.0,
        0.0,
    );

    // --- iMouse, via egui's pointer (logical points -> physical pixels).
    // egui's pointer origin is top-left; shadertoy's iMouse is bottom-left,
    // so y is flipped to match (the `package::main` wrapper flips
    // frag_coord the same way). Only presses on the bare canvas count.
    if let Ok(ctx) = p.contexts.ctx_mut() {
        let ppp = ctx.pixels_per_point();
        let (hover, pressed, held) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });
        let on_canvas = hover.is_some_and(|pos| pointer_on_canvas(ctx, p.state.canvas, pos));
        let pos = hover.map(|pos| {
            let px = pos * ppp;
            Vec2::new(px.x, resolution.y - px.y)
        });
        track_mouse(&mut p.state, pos, pressed && on_canvas, held);

        // Canvas rows under the toolbar and status bar, for the debugger
        // overlay's layout.
        if p.state.canvas.is_positive() {
            p.uniforms.overlay_top = p.state.canvas.min.y * ppp;
            p.uniforms.overlay_bottom = (resolution.y - p.state.canvas.max.y * ppp).max(0.0);
        }
    }

    p.uniforms.time = p.state.time;
    // Shadertoy time: frozen while paused, scaled by the speed slider.
    p.uniforms.time_delta = step;
    p.uniforms.resolution = resolution;
    p.uniforms.mouse = p.state.mouse;
    p.uniforms.date = date_uniforms();
    p.uniforms.sample_rate = SAMPLE_RATE_HZ;
    p.uniforms.frame_rate = p.state.fps;
    // Indexed by the latest compile's slots; the shader still on screen
    // until that one's pipelines are built may misread them for a frame or
    // two if the list of constants changed.
    p.uniforms.tweaks = p.tweaks.values;
    p.debugger_on
        .set_if_neq(debugger::DebuggerOn(p.state.debugger));
    // i_frame and the buffer channels' clocks and sizes belong to the shader
    // on screen, so the render world fills them in (`render::active`).
}

/// Everything [`compile_and_apply`] reads or writes, bundled into one parameter.
#[derive(SystemParam)]
struct CompileParams<'w, 's> {
    run: ResMut<'w, RunRequest>,
    status: ResMut<'w, CompileStatus>,
    doc: Res<'w, ShaderDoc>,
    shaders: ResMut<'w, Assets<Shader>>,
    current: ResMut<'w, CurrentShader>,
    fragment: Single<'w, 's, &'static mut ShaderToyFragment>,
    feedback: Res<'w, ShaderFeedback>,
    tweaks: ResMut<'w, Tweaks>,
}

/// Compile the editor text and, on success, hot-swap the running shader.
/// On error the previous shader keeps running and the message lands in
/// [`CompileStatus`] for the UI.
fn compile_and_apply(mut p: CompileParams) {
    if !p.run.0 {
        return;
    }
    p.run.0 = false;
    p.status.note = None;

    let started = Instant::now();
    match compile::compile_user_shader(&p.doc.text) {
        Ok(compiled) => {
            // Replace the slot that is not on screen. Repeated compiles
            // before the first lands keep overwriting the same pending slot.
            let active = p.feedback.lock().active.as_ref().map(|f| f.handle.id());
            let slot = p
                .current
                .slots
                .iter()
                .position(|h| Some(h.id()) != active && *h == p.fragment.handle)
                .unwrap_or_else(|| usize::from(Some(p.current.slots[0].id()) == active));
            let handle = p.current.slots[slot].clone();
            let path = format!("shadertoy://user/{slot}");
            p.shaders
                .insert(&handle, Shader::from_wgsl(compiled.wgsl.clone(), path))
                .expect("slot handles stay valid");
            p.current.generation += 1;
            p.current.wgsl = compiled.wgsl;
            // Start from the values in the source (which already hold any
            // tweak that was written back).
            for (slot, c) in compiled.tweaks.iter().enumerate() {
                p.tweaks.values[slot] = Vec4::from_array(c.value);
            }
            p.tweaks.constants = compiled.tweaks;
            p.tweaks.fixed = compiled.fixed;
            p.tweaks.dirty = 0;
            // The render world switches to it (and restarts i_frame, the
            // channel clocks and the buffers) once its pipelines are built.
            **p.fragment = ShaderToyFragment {
                handle,
                buffers: compiled.buffers,
                generation: p.current.generation,
            };
            let ms = started.elapsed().as_secs_f32() * 1000.0;
            p.status.error = None;
            p.status.last_ms = Some(ms);
            info!("shader compiled and applied in {ms:.1} ms");
        }
        Err(e) => {
            let msg = e.to_string();
            warn!(
                "shader compile failed: {}",
                msg.lines().next().unwrap_or("error")
            );
            p.status.error = Some(msg);
        }
    }
}

/// Report a requested shader whose pipelines failed to build (the render
/// world never activates it) and go back to requesting what is on screen.
fn poll_render_feedback(
    feedback: Res<ShaderFeedback>,
    mut fragment: Single<&mut ShaderToyFragment>,
    mut status: ResMut<CompileStatus>,
) {
    let mut fb = feedback.lock();
    let Some((generation, error)) = fb.rejected.take() else {
        return;
    };
    if generation != fragment.generation {
        return; // stale: something newer was requested since
    }
    warn!("GPU rejected the shader: {error}");
    status.error = Some(format!("The GPU rejected the shader:\n{error}"));
    if let Some(active) = fb.active.clone() {
        **fragment = active;
    }
}

/// Bevy's default answer to a GPU (wgpu) error is to quit, which natively
/// loses unsaved edits. A validation or internal error here almost always
/// comes from the user's shader: naga accepted it, but wgpu or the WebGL
/// backend rejects the pipeline (a GLSL translation failure is reported as
/// internal). So blame the newest shader that is not yet on screen (else the
/// one on screen), go back to the last good one (else the black
/// placeholder), say so in the status bar, and carry on. Out-of-memory and
/// device loss, and errors the placeholder still produces, keep the default.
fn on_render_error(
    error: &RenderError,
    main_world: &mut World,
    render_world: &mut World,
) -> RenderErrorPolicy {
    let quit = |main_world: &mut World| {
        error!("Quitting the application due to {:?} RenderError", error.ty);
        main_world.write_message(AppExit::error());
        RenderErrorPolicy::StopRendering
    };
    if !matches!(error.ty, ErrorType::Validation | ErrorType::Internal) {
        return quit(main_world);
    }
    let mut views = render_world.query::<(&mut ShaderToyFragment, Option<&ActiveShader>)>();
    let Some((requested, active)) = views.iter(render_world).next() else {
        return quit(main_world);
    };
    let active = active.map(|a| a.fragment.clone());
    let placeholder = main_world
        .resource::<CurrentShader>()
        .placeholder_fragment();
    let (culprit, fallback) = match active {
        Some(a) if a.generation != requested.generation => (requested.generation, a),
        Some(a) if a.generation != 0 => (a.generation, placeholder),
        None if requested.generation != 0 => (requested.generation, placeholder),
        _ => return quit(main_world),
    };

    render_world.resource_mut::<RejectedGeneration>().0 = Some(culprit);
    for (mut requested, _) in views.iter_mut(render_world) {
        *requested = fallback.clone();
    }
    let mut cameras = main_world.query::<&mut ShaderToyFragment>();
    for mut requested in cameras.iter_mut(main_world) {
        *requested = fallback.clone();
    }
    main_world.resource_mut::<CompileStatus>().error = Some(format!(
        "The GPU rejected the shader:\n{}",
        error.description
    ));
    RenderErrorPolicy::Ignore
}

/// Write the document to browser storage whenever it changes: edits,
/// opened examples and opened files alike (a no-op natively, where Ctrl+S
/// saves). Comparing against the last saved text is cheap next to a frame
/// and catches every path that changes the document.
fn autosave(doc: Res<ShaderDoc>, mut saved: Local<Option<String>>) {
    match saved.as_deref() {
        // The startup document (an example or the restored save) is not an edit.
        None => *saved = Some(doc.text.clone()),
        Some(text) if text == doc.text => {}
        Some(_) => {
            persist::save_doc(&doc.text);
            *saved = Some(doc.text.clone());
        }
    }
}

/// Everything the [`ui`] system reads or writes, bundled into one parameter.
#[derive(SystemParam)]
struct UiParams<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    doc: ResMut<'w, ShaderDoc>,
    state: ResMut<'w, ShaderToyState>,
    status: ResMut<'w, CompileStatus>,
    run: ResMut<'w, RunRequest>,
    wins: ResMut<'w, UiWindows>,
    settings: ResMut<'w, Settings>,
    current: Res<'w, CurrentShader>,
    highlight: ResMut<'w, editor::HighlightCache>,
    analyzer: Res<'w, analyzer::Analyzer>,
    playback: ResMut<'w, Playback>,
    blur: ResMut<'w, render::blur::Blur>,
    tweaks: ResMut<'w, Tweaks>,
    browser: ResMut<'w, browser::Browser>,
    evolve: ResMut<'w, evolve::Evolve>,
    preview_feedback: Res<'w, render::previews::PreviewFeedback>,
    ui_texture: ResMut<'w, ui_texture::UiTexture>,
    /// egui's id for the blurred image, registered on the first frame.
    blur_texture: Local<'s, Option<egui::TextureId>>,
}

/// Keep wgsl-analyzer (live diagnostics) in sync with the document and the
/// settings toggle.
fn run_analyzer(
    mut analyzer: ResMut<analyzer::Analyzer>,
    doc: Res<ShaderDoc>,
    settings: Res<Settings>,
) {
    analyzer.update(settings.analyzer, &doc.text);
}

/// All egui UI: toolbar, editor window, error panel, WGSL viewer.
fn ui(mut p: UiParams) {
    // Register the blurred backdrop with egui once: resizing replaces the
    // image behind the same handle, so the id stays valid.
    let blur_texture = *p.blur_texture.get_or_insert_with(|| {
        p.contexts
            .add_image(bevy_egui::EguiTextureHandle::Weak(p.blur.image.id()))
    });
    let Ok(ctx) = p.contexts.ctx_mut() else {
        return;
    };
    theme::apply(ctx);
    // No frosted glass while the UI goes through the shader (see `ui_texture`).
    let blur = if p.ui_texture.on {
        0.0
    } else {
        p.settings.blur
    };
    let sigma = blur * ctx.pixels_per_point();
    if p.blur.sigma != sigma {
        p.blur.sigma = sigma;
    }
    let glass = (blur > 0.0).then(|| Glass {
        texture: blur_texture,
        screen: ctx.viewport_rect(),
    });

    // Global shortcuts: Ctrl/Cmd+Enter compiles, Ctrl/Cmd+S saves,
    // Ctrl/Cmd+O opens. (index.html suppresses the browser's own S/O dialogs.)
    let (run, save, open) = ctx.input(|i| {
        let cmd = i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt;
        (
            i.modifiers.command && i.key_pressed(egui::Key::Enter),
            cmd && i.key_pressed(egui::Key::S),
            cmd && i.key_pressed(egui::Key::O),
        )
    });
    if run {
        p.run.0 = true;
    }
    if save {
        p.status.note = match persist::save_now(&p.doc.text) {
            Ok(location) => Some((format!("saved to {location}"), true)),
            Err(e) => Some((format!("save failed: {e}"), false)),
        };
    }
    if open {
        match persist::open_now() {
            Ok(Some(text)) => open_into(
                &mut p.doc,
                &mut p.state,
                &mut p.run,
                &mut p.status,
                text,
                "shader.wesl",
            ),
            // Web: the file dialog is open; the text lands via the queue.
            Ok(None) => {}
            Err(e) => p.status.note = Some((e, false)),
        }
    }
    // Files chosen through the web file dialog arrive here once read.
    for (name, text) in persist::take_opened() {
        open_into(
            &mut p.doc,
            &mut p.state,
            &mut p.run,
            &mut p.status,
            text,
            &name,
        );
    }

    // egui 0.36 attaches panels to a `Ui`, not to the `Context`: build the
    // root Ui that spans the viewport, like bevy_egui's own examples do.
    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("root_panels"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // --- toolbar -----------------------------------------------------------
    show_panel(&mut root_ui, glass, egui::Panel::top("toolbar"), |ui| {
        ui.horizontal(|ui| {
            if ui
                .button("Compile WESL Shader")
                .on_hover_text("Compile & apply (Ctrl+Enter)")
                .clicked()
            {
                p.run.0 = true;
            }
            if ui
                .button(icon(lucide_icons::Icon::RotateCcw))
                .on_hover_text("Reset time, frame counter and buffers")
                .clicked()
            {
                p.state.time = 0.0;
                p.playback.epoch = p.playback.epoch.wrapping_add(1);
            }
            ui.add(
                egui::Slider::new(&mut p.state.speed, 0.0..=4.0)
                    .text("speed")
                    .max_decimals(2),
            )
            .on_hover_text("Clock speed; 0 pauses");
            // Fixed-width slots: digit changes must not reflow the toolbar.
            readout(ui, format!("t = {:.1}s", p.state.time), 84.0);
            readout(ui, format!("{:.0} fps", p.state.fps), 48.0);

            ui.separator();

            ui.toggle_value(&mut p.wins.examples, "Examples")
                .on_hover_text("Browse the examples, running live");
            ui.toggle_value(&mut p.wins.editor, "Code");
            ui.toggle_value(&mut p.wins.wgsl, "WGSL");
            ui.toggle_value(&mut p.wins.constants, "Constants")
                .on_hover_text("Tweak the shader's constants live");
            ui.toggle_value(&mut p.evolve.on, "Evolve").on_hover_text(
                "Breed the shader's constants: pick from nine variants (Escape leaves)",
            );
            ui.toggle_value(&mut p.state.debugger, icon(lucide_icons::Icon::Bug))
                .on_hover_text("In-shader debugger: crosshair, probe boxes, fps");
            ui_texture::toolbar_toggle(ui, &mut p.ui_texture, &p.current.wgsl);
            ui.toggle_value(&mut p.wins.settings, icon(lucide_icons::Icon::Settings))
                .on_hover_text("Settings");
            ui.toggle_value(&mut p.wins.about, "?")
                .on_hover_text("About");
        });
    });

    // --- status / error panel ----------------------------------------------
    status_bar(
        &mut root_ui,
        glass,
        &p.status,
        &mut p.wins.errors_expanded,
        &p.analyzer,
    );

    // --- canvas: what the panels leave free shows the shader ----------------
    p.state.canvas = root_ui.available_rect_before_wrap();
    evolve::show(
        ctx,
        &mut p.evolve,
        &mut p.tweaks,
        p.state.canvas,
        p.current.generation,
        &p.preview_feedback,
    );

    // --- editor window -------------------------------------------------------
    if p.wins.editor {
        show_window(
            ctx,
            glass,
            "code_window",
            egui::Window::new("Code — package::user")
                .default_pos([12.0, 44.0])
                .default_size([640.0, 460.0])
                .open(&mut p.wins.editor),
            |ui| {
                editor::show_code_editor(
                    ui,
                    &mut p.doc.text,
                    &mut p.highlight,
                    p.analyzer.diagnostics(),
                );
            },
        );
    }

    // --- generated WGSL viewer -----------------------------------------------
    if p.wins.wgsl {
        show_window(
            ctx,
            glass,
            "wgsl_window",
            egui::Window::new("Generated WGSL — package::main")
                .default_pos([680.0, 60.0])
                .default_size([520.0, 480.0])
                .open(&mut p.wins.wgsl),
            |ui| {
                editor::show_code_view(ui, &p.current.wgsl, &mut p.highlight);
            },
        );
    }

    // --- constants window ----------------------------------------------------
    if p.wins.constants {
        show_window(
            ctx,
            glass,
            "constants_window",
            egui::Window::new("Constants")
                .default_pos([680.0, 60.0])
                .default_size([340.0, 300.0])
                .open(&mut p.wins.constants),
            |ui| constants_ui(ui, &mut p.tweaks),
        );
    }
    // Write tweaks into the source once the pointer lets go, so a drag
    // edits the text once, not every frame.
    if p.tweaks.dirty != 0 && !ctx.input(|i| i.pointer.any_down()) {
        let tweaks = &mut *p.tweaks;
        for (slot, c) in tweaks.constants.iter().enumerate() {
            if tweaks.dirty & (1 << slot) != 0
                && !tweak::write_back(&mut p.doc.text, c, tweaks.values[slot].to_array())
            {
                let note = format!("{} is no longer in the code; compile to refresh", c.name);
                p.status.note = Some((note, false));
            }
        }
        tweaks.dirty = 0;
    }

    // --- examples window (after the others: on top when they open together) -----------------------------------------------------
    p.browser.begin_frame();
    if p.wins.examples {
        let mut picked = None;
        // Pinned to 90% of the canvas every frame, so it follows window
        // resizes (and the first frame's provisional canvas size).
        let rect =
            egui::Rect::from_center_size(p.state.canvas.center(), p.state.canvas.size() * 0.9);
        show_window(
            ctx,
            glass,
            "examples_window",
            egui::Window::new("Examples")
                .fixed_rect(rect)
                .collapsible(false)
                .open(&mut p.wins.examples),
            |ui| {
                picked = browser::show(ui, &mut p.browser, &p.preview_feedback, p.state.example);
            },
        );
        if let Some(example) = picked {
            // Loading replaces the editor text (and its autosave): ask unless
            // it is unchanged from an example.
            if examples::ALL.iter().any(|e| e.code == p.doc.text) {
                load_example(&mut p.doc, &mut p.state, &mut p.run, example);
            } else {
                p.wins.confirm_example = Some(example);
            }
        }
    }
    if let Some(example) = p.wins.confirm_example {
        let modal = egui::Modal::new(egui::Id::new("confirm_example")).show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.strong(format!("Open {}?", example.name));
            ui.label(
                "This replaces the code in the editor, and your edits are not \
                 saved anywhere else. Ctrl+S saves them to a file first.",
            );
            ui.horizontal(|ui| {
                if ui.button("Replace").clicked() {
                    load_example(&mut p.doc, &mut p.state, &mut p.run, example);
                    ui.close();
                }
                if ui.button("Cancel").clicked() {
                    ui.close();
                }
            });
        });
        if modal.should_close() {
            p.wins.confirm_example = None;
        }
    }

    // --- settings window -----------------------------------------------------
    if p.wins.settings {
        show_window(
            ctx,
            glass,
            "settings_window",
            egui::Window::new("Settings")
                .default_pos([12.0, 60.0])
                .default_size([300.0, 150.0])
                .open(&mut p.wins.settings),
            |ui| {
                let before = (
                    p.settings.random_example,
                    p.settings.analyzer,
                    p.settings.blur,
                );
                ui.checkbox(
                    &mut p.settings.random_example,
                    "random example on fresh start",
                );
                ui.checkbox(&mut p.settings.analyzer, "live diagnostics (wgsl-analyzer)");
                if let analyzer::Status::Unavailable(why) = p.analyzer.status() {
                    ui.label(egui::RichText::new(why).small().weak());
                }
                ui.add(
                    egui::Slider::new(&mut p.settings.blur, 0.0..=MAX_BLUR)
                        .text("frosted glass blur")
                        .suffix(" px")
                        .max_decimals(0),
                )
                .on_hover_text(
                    "How far the shader behind the UI is blurred (Gaussian sigma); 0 turns the glass off",
                );
                ui.separator();
                ui.weak("Ctrl+S saves · Ctrl+O opens a .wesl file");
                let after = (
                    p.settings.random_example,
                    p.settings.analyzer,
                    p.settings.blur,
                );
                if after != before {
                    persist::save_settings(&p.settings.encode());
                }
            },
        );
    }

    // --- about window ----------------------------------------------------------
    if p.wins.about {
        show_window(
            ctx,
            glass,
            "about_window",
            egui::Window::new("About wesl-shadertoy")
                .default_pos([12.0, 60.0])
                .default_size([380.0, 330.0])
                .open(&mut p.wins.about),
            |ui| {
                ui.label(
                    "A classic shadertoy-style live shader playground for WESL: one \
                     animated fullscreen fragment shader, one code editor.",
                );
                ui.separator();
                // Static version text — keep in sync with Cargo.toml (Bevy 0.20
                // ships no BEVY_VERSION constant to read instead).
                ui.label("Bevy 0.20 · bevy_egui 0.43 (egui 0.36) · WESL 0.6 · naga 30");
                ui.separator();
                ui.hyperlink_to("repository", "https://github.com/barafael/wesl-shadertoy");
                ui.hyperlink_to("live demo", "https://barafael.github.io/wesl-shadertoy/");
                ui.hyperlink_to("WESL language", "https://wesl-lang.dev");
                ui.hyperlink_to("shadertoy.com", "https://www.shadertoy.com");
                ui.separator();
                ui.label("MIT OR Apache-2.0.");
                ui.weak(
                    "fullscreen_material.rs vendored from Bevy 0.20 (MIT OR Apache-2.0) \
                     · live diagnostics by wgsl-analyzer (MIT OR Apache-2.0) \
                     · debugger overlay ported from eliotbo/bevy_shadertoy_wgsl \
                     (MIT OR Apache-2.0) \
                     · coi-serviceworker by Guido Zuidhof (MIT) \
                     · Gruvbox colors by morhetz · examples ported from shadertoy \
                     classics (each credits its author and license in its header) \
                     · LYGIA by Patricio Gonzalez Vivo and contributors (Prosperity \
                     License 3.0.0 or Patron License).",
                );
            },
        );
    }
}

/// Frosted glass: the blurred shader image ([`render::blur`]) painted under
/// a window or panel, with the theme's fill made translucent on top.
#[derive(Clone, Copy)]
struct Glass {
    texture: egui::TextureId,
    /// The viewport the blurred image covers (egui points).
    screen: egui::Rect,
}

impl Glass {
    /// How much of the theme's fill covers the blurred backdrop. Less makes
    /// comments and weak text unreadable over bright shaders.
    const OPACITY: f32 = 0.9;

    fn shape(self, rect: egui::Rect, fill: egui::Color32) -> egui::Shape {
        let to_uv = |p: egui::Pos2| ((p - self.screen.min) / self.screen.size()).to_pos2();
        let uv = egui::Rect::from_min_max(to_uv(rect.min), to_uv(rect.max));
        egui::Shape::Vec(vec![
            egui::Shape::image(self.texture, rect, uv, egui::Color32::WHITE),
            egui::Shape::rect_filled(rect, 0.0, fill.gamma_multiply(Self::OPACITY)),
        ])
    }
}

/// Show `window` under the id `id`, on frosted `glass` if given: its own
/// fill turns transparent and the glass is painted first in its layer,
/// sized to the window once it is laid out.
fn show_window(
    ctx: &egui::Context,
    glass: Option<Glass>,
    id: &str,
    window: egui::Window,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let id = egui::Id::new(id);
    let window = window.id(id);
    let Some(glass) = glass else {
        window.show(ctx, add_contents);
        return;
    };
    let fill = ctx.global_style().visuals.window_fill;
    let frame = egui::Frame::window(&ctx.global_style()).fill(egui::Color32::TRANSPARENT);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, id));
    let backdrop = painter.add(egui::Shape::Noop);
    if let Some(shown) = window.frame(frame).show(ctx, add_contents) {
        painter.set(backdrop, glass.shape(shown.response.rect, fill));
    }
}

/// [`show_window`] for a panel of the root `ui`.
fn show_panel(
    ui: &mut egui::Ui,
    glass: Option<Glass>,
    panel: egui::Panel,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let Some(glass) = glass else {
        panel.show(ui, add_contents);
        return;
    };
    let fill = ui.visuals().panel_fill;
    let frame = egui::Frame::side_top_panel(ui.style()).fill(egui::Color32::TRANSPARENT);
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let shown = panel.frame(frame).show(ui, add_contents);
    ui.painter()
        .set(backdrop, glass.shape(shown.response.rect, fill));
}

/// The status bar: compile result or note on the left, wgsl-analyzer on the
/// right, and below them the full compile error while it is expanded.
///
/// Canvas clicks through empty bar space are already ignored: the canvas
/// rect excludes the panels. Two panel ids, so each keeps its own size: the
/// status line always fits its one row, while the expanded error panel is
/// resizable and remembers the height it was dragged to.
fn status_bar(
    root_ui: &mut egui::Ui,
    glass: Option<Glass>,
    status: &CompileStatus,
    errors_expanded: &mut bool,
    analyzer: &analyzer::Analyzer,
) {
    let expanded_error = status.error.as_deref().filter(|_| *errors_expanded);
    let panel = match expanded_error {
        Some(_) => egui::Panel::bottom("status_errors")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0),
        None => egui::Panel::bottom("status").resizable(false),
    };
    show_panel(root_ui, glass, panel, |ui| {
        ui.horizontal(|ui| {
            if let Some(err) = &status.error {
                let summary = err.lines().next().unwrap_or("error");
                if ui
                    .selectable_label(
                        *errors_expanded,
                        format!("{} compile failed", icon(lucide_icons::Icon::CircleX)),
                    )
                    .on_hover_text("The previous shader keeps running")
                    .clicked()
                {
                    *errors_expanded = !*errors_expanded;
                }
                ui.label(egui::RichText::new(summary).color(theme::ERR_COLOR));
            } else if let Some((note, ok)) = &status.note {
                let color = if *ok {
                    theme::OK_COLOR
                } else {
                    theme::ERR_COLOR
                };
                ui.label(egui::RichText::new(note.clone()).color(color));
            } else if let Some(ms) = status.last_ms {
                ui.label(
                    egui::RichText::new(format!(
                        "{} compiled in {ms:.1} ms",
                        icon(lucide_icons::Icon::Check)
                    ))
                    .color(theme::OK_COLOR),
                );
            } else {
                ui.label("not compiled yet");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                analyzer_readout(ui, analyzer);
            });
        });
        if let Some(mut err) = expanded_error {
            ui.push_id("error_text", |ui| {
                // Fills the panel, so dragging its edge resizes the text.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut err)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .min_size(ui.available_size()),
                        );
                    });
            });
        }
    });
}

/// The Constants window: a control per live-tweakable constant. Edits go
/// straight to [`Tweaks::values`] (uploaded as uniforms, so the shader
/// follows without a recompile) and mark the slot dirty for write-back.
fn constants_ui(ui: &mut egui::Ui, tweaks: &mut Tweaks) {
    if tweaks.constants.is_empty() {
        ui.label(
            "No tweakable constants. Module-level constants with literal values \
             show up here after a compile, e.g.",
        );
        ui.code("const SPEED: f32 = 1.0;");
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("constants_grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for (slot, c) in tweaks.constants.iter().enumerate() {
                        ui.monospace(&c.name);
                        let value = &mut tweaks.values[slot];
                        let before = *value;
                        constant_control(ui, c, value);
                        let default = Vec4::from_array(c.value);
                        if ui
                            .add_enabled(
                                *value != default,
                                egui::Button::new(icon(lucide_icons::Icon::RotateCcw)),
                            )
                            .on_hover_text("Back to the value it was compiled with")
                            .clicked()
                        {
                            *value = default;
                        }
                        if *value != before {
                            tweaks.dirty |= 1 << slot;
                        }
                        ui.end_row();
                    }
                });
        });
    }
    ui.separator();
    if !tweaks.fixed.is_empty() {
        ui.weak(format!(
            "Fixed (WGSL needs them as true constants): {}",
            tweaks.fixed.join(", ")
        ));
    }
    ui.weak(
        "Changes apply live and are written into the code on release. \
         Annotate a constant with `// tweak: 0..2`, `// tweak: color` or \
         `// tweak: off`.",
    );
}

/// The control for one constant: a slider for numbers, a color picker for
/// colors, one drag field per component for other vectors.
fn constant_control(ui: &mut egui::Ui, c: &tweak::Constant, value: &mut Vec4) {
    use tweak::Kind;
    match c.kind {
        Kind::F32 => {
            let (lo, hi) = c.range(c.value[0]);
            ui.add(egui::Slider::new(&mut value.x, lo..=hi).clamping(egui::SliderClamping::Never));
        }
        Kind::I32 | Kind::U32 => {
            let (lo, hi) = c.range(c.value[0]);
            let mut n = value.x.round() as i64;
            let slider = egui::Slider::new(&mut n, lo as i64..=hi as i64)
                .clamping(egui::SliderClamping::Never);
            if ui.add(slider).changed() {
                let min = if c.kind == Kind::U32 { 0 } else { i64::MIN };
                value.x = n.max(min) as f32;
            }
        }
        // egui's color button round-trips the color through its own
        // representation every frame, nudging it by float noise: only a
        // real edit may change the value (and mark it for write-back).
        Kind::Vec(3) if c.is_color() => {
            let mut rgb = value.truncate().to_array();
            ui.color_edit_button_rgb(&mut rgb);
            let picked = Vec3::from_array(rgb).extend(0.0);
            if !picked.abs_diff_eq(*value, 1e-4) {
                *value = picked;
            }
        }
        Kind::Vec(4) if c.is_color() => {
            let mut rgba = value.to_array();
            ui.color_edit_button_rgba_unmultiplied(&mut rgba);
            let picked = Vec4::from_array(rgba);
            if !picked.abs_diff_eq(*value, 1e-4) {
                *value = picked;
            }
        }
        Kind::Vec(n) => {
            ui.horizontal(|ui| {
                for i in 0..n as usize {
                    let mut field = egui::DragValue::new(&mut value[i]);
                    field = match c.hint {
                        // An annotated range: drag across it in ~200 px.
                        tweak::Hint::Range(lo, hi) => {
                            field.range(lo..=hi).speed(((hi - lo) / 200.0) as f64)
                        }
                        _ => field.speed((c.value[i].abs().max(0.1) * 0.01) as f64),
                    };
                    ui.add(field);
                }
            });
        }
    }
}

/// wgsl-analyzer's state at the right end of the status bar: problem counts
/// while it runs, otherwise why not (the details are in the hover text).
fn analyzer_readout(ui: &mut egui::Ui, analyzer: &analyzer::Analyzer) {
    use analyzer::{Severity, Status};
    // Starting / unavailable is not the shader's fault: weak, not red.
    let weak = ui.visuals().weak_text_color();
    let (text, color, hover) = match analyzer.status() {
        Status::Off => return,
        Status::Starting => ("starting…".to_owned(), weak, "Loading the language server"),
        Status::Unavailable(why) => ("unavailable".to_owned(), weak, why.as_str()),
        Status::Ready => {
            let diagnostics = analyzer.diagnostics();
            let count = |s| {
                let n = diagnostics.iter().filter(|d| d.severity == s).count();
                (n > 0).then_some(n)
            };
            let hover = "Live diagnostics: hover the squiggles in the code";
            let plural = |n: usize, what| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
            match (count(Severity::Error), count(Severity::Warning)) {
                (Some(e), Some(w)) => {
                    let text = format!("{}, {}", plural(e, "error"), plural(w, "warning"));
                    (text, theme::ERR_COLOR, hover)
                }
                (Some(e), None) => (plural(e, "error"), theme::ERR_COLOR, hover),
                (None, Some(w)) => (plural(w, "warning"), ui.visuals().warn_fg_color, hover),
                (None, None) => (icon(lucide_icons::Icon::Check), theme::OK_COLOR, hover),
            }
        }
    };
    let label = egui::RichText::new(format!("wgsl-analyzer: {text}")).color(color);
    ui.label(label).on_hover_text(hover);
}

/// Whether a pointer at `pos` (egui points) is over the bare shader canvas:
/// inside the area the panels leave free and not under any window or popup.
/// Uses last frame's layers, like egui's own hit testing. (The previous test,
/// `interact_pos().is_some()`, is true wherever the pointer is, so iMouse
/// never saw a click.)
fn pointer_on_canvas(ctx: &egui::Context, canvas: egui::Rect, pos: egui::Pos2) -> bool {
    canvas.contains(pos)
        && ctx
            .layer_id_at(pos)
            .is_none_or(|layer| layer.order == egui::Order::Background)
}

/// shadertoy's iMouse state machine. `pos`: the pointer in physical pixels,
/// bottom-left origin (`None` while it is outside the window); `press`: a
/// primary press on the canvas this frame; `held`: the primary button is
/// down. xy follows the pointer while held, zw is the press position,
/// negated once released.
fn track_mouse(state: &mut ShaderToyState, pos: Option<Vec2>, press: bool, held: bool) {
    if press && let Some(at) = pos {
        state.mouse = Vec4::new(at.x, at.y, at.x, at.y);
        state.mouse_held = true;
    } else if state.mouse_held {
        if let Some(at) = pos {
            state.mouse.x = at.x;
            state.mouse.y = at.y;
        }
        // `!held` rather than a release event, so a press and release
        // within one frame still ends the drag on the next.
        if !held {
            state.mouse_held = false;
            state.mouse.z = -state.mouse.z;
            state.mouse.w = -state.mouse.w;
        }
    }
}

/// Load an opened file into the editor and compile it. Takes the individual
/// fields (not `&mut UiParams`) so it can run while `ctx` borrows `p.contexts`.
fn open_into(
    doc: &mut ShaderDoc,
    state: &mut ShaderToyState,
    run: &mut RunRequest,
    status: &mut CompileStatus,
    text: String,
    name: &str,
) {
    doc.text = text;
    state.example = None;
    run.0 = true;
    status.note = Some((format!("opened {name}"), true));
}

/// Put `example` in the editor and compile it.
fn load_example(
    doc: &mut ShaderDoc,
    state: &mut ShaderToyState,
    run: &mut RunRequest,
    example: &'static examples::Example,
) {
    doc.text = example.code.to_string();
    state.example = Some(example.name);
    run.0 = true;
}

/// Native-only: `--shader <file>` seeds the editor with the contents of a
/// WGSL file, so scripted live tests can inject a document without GUI
/// input. Takes precedence over saved documents and examples. Web builds
/// have no argv and always return `None`.
fn cli_shader() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--shader"
                && let Some(path) = args.next()
            {
                match std::fs::read_to_string(&path) {
                    Ok(text) => return Some(text),
                    Err(e) => {
                        warn!("--shader {path}: {e}; starting with the default example");
                        return None;
                    }
                }
            }
        }
        None
    }
}

/// Native-only: `--example <name>` picks the startup example
/// (case-insensitive, see [`examples::by_name`]), so scripted screenshot runs
/// can pin a deterministic shader. Web builds have no argv and always return
/// `None`.
fn cli_example() -> Option<&'static examples::Example> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--example"
                && let Some(name) = args.next()
            {
                return examples::by_name(&name);
            }
        }
        None
    }
}

/// Native-only: whether `flag` was passed. `--debugger` starts with the
/// in-shader debugger overlay on (same as the toolbar bug icon) and
/// `--examples` with the Examples window open, so scripted live tests can
/// show them.
fn cli_flag(flag: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = flag;
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::args().any(|a| a == flag)
    }
}

/// Time-seeded xorshift pick — no `rand` dependency for one roll per start.
fn random_example() -> &'static examples::Example {
    let mut x = (now_epoch_secs() * 1.0e6) as u64;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let idx = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % examples::ALL.len();
    &examples::ALL[idx]
}

/// A Lucide icon as a one-char string; [`install_fonts`] registers the font
/// that maps these private-use codepoints to glyphs.
fn icon(i: lucide_icons::Icon) -> String {
    char::from(i).to_string()
}

/// A label in a fixed-width slot, so changing digits never reflows the toolbar.
fn readout(ui: &mut egui::Ui, text: String, width: f32) {
    ui.add_sized(
        [width, ui.spacing().interact_size.y],
        egui::Label::new(text).wrap_mode(egui::TextWrapMode::Truncate),
    );
}

/// Howard Hinnant's `civil_from_days`, so iDate needs no chrono dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // days since Mar 1
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

// --- wall clock ---------------------------------------------------------------

/// Seconds since the Unix epoch. `SystemTime::now()` *panics* on
/// wasm32-unknown-unknown, so the browser build asks JavaScript instead.
pub(crate) fn now_epoch_secs() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

/// x: year, y: month (1-12), z: day of month, w: seconds since midnight.
/// (shadertoy's `iDate` convention; matches the `i_date` docs in the
/// environment module.)
fn date_uniforms() -> Vec4 {
    let secs = now_epoch_secs();
    let days = (secs / 86400.0).floor() as i64;
    let seconds_of_day = secs - days as f64 * 86400.0;
    let (year, month, day) = civil_from_days(days);
    Vec4::new(year as f32, month as f32, day as f32, seconds_of_day as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toolbar and status bar render Lucide icons as private-use-area
    /// codepoints; guard that the font setup behind [`install_fonts`] can
    /// actually resolve every icon we use, or they would show as tofu.
    /// A press over a window is UI, a press on the free viewport is canvas.
    #[test]
    fn pointer_on_canvas_excludes_windows() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let canvas = screen.shrink2(egui::vec2(0.0, 30.0)); // minus toolbar/status
        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let empty_spot = egui::pos2(700.0, 500.0);
        // Windows only become hit-testable once laid out, so run a few frames.
        for _ in 0..3 {
            ctx.run_ui(input(vec![egui::Event::PointerMoved(empty_spot)]), |ui| {
                egui::Window::new("w")
                    .fixed_pos([100.0, 100.0])
                    .fixed_size([200.0, 150.0])
                    .show(ui.ctx(), |ui| ui.label("x"));
            })
            .drop_without_applying_deltas();
        }

        assert!(pointer_on_canvas(&ctx, canvas, empty_spot));
        let (on_window, on_toolbar) = (egui::pos2(150.0, 150.0), egui::pos2(700.0, 10.0));
        assert!(!pointer_on_canvas(&ctx, canvas, on_window), "window");
        assert!(!pointer_on_canvas(&ctx, canvas, on_toolbar), "toolbar");
        // The old test called this empty spot UI.
        assert!(ctx.input(|i| i.pointer.interact_pos().is_some()));
    }

    /// The status bar grows to show an expanded compile error and shrinks
    /// back to its one row once the error is fixed (it used to keep the
    /// error's height, and grew too little to show the error).
    #[test]
    fn status_bar_fits_its_content() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let analyzer = analyzer::Analyzer::default();
        let mut expanded = true;
        // The height the bar takes from the bottom of the screen.
        let mut bar_height = |status: &CompileStatus| {
            let mut canvas = egui::Rect::NOTHING;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                ctx.run_ui(input, |ui| {
                    status_bar(ui, None, status, &mut expanded, &analyzer);
                    canvas = ui.available_rect_before_wrap();
                })
                .drop_without_applying_deltas();
            }
            screen.max.y - canvas.max.y
        };

        let ok = CompileStatus {
            last_ms: Some(1.0),
            ..default()
        };
        let row = bar_height(&ok);
        assert!(row < 40.0, "status line is {row} tall");
        let failed = CompileStatus {
            error: Some((1..=12).map(|i| format!("line {i}\n")).collect()),
            ..default()
        };
        let open = bar_height(&failed);
        assert!(open >= 200.0, "expanded error is only {open} tall");
        assert_eq!(bar_height(&ok), row, "the bar shrinks back after a fix");
    }

    #[test]
    fn imouse_press_drag_release() {
        let mut s = ShaderToyState::default();
        let at = |x, y| Some(Vec2::new(x, y));

        track_mouse(&mut s, at(10.0, 20.0), false, true);
        assert_eq!(s.mouse, Vec4::ZERO, "drag begun on the UI is not ours");

        track_mouse(&mut s, at(10.0, 20.0), true, true);
        assert_eq!(s.mouse, Vec4::new(10.0, 20.0, 10.0, 20.0));
        track_mouse(&mut s, at(30.0, 40.0), false, true);
        assert_eq!(s.mouse, Vec4::new(30.0, 40.0, 10.0, 20.0));
        track_mouse(&mut s, None, false, true); // pointer left the window
        assert_eq!(s.mouse, Vec4::new(30.0, 40.0, 10.0, 20.0));
        track_mouse(&mut s, at(35.0, 45.0), false, false);
        assert_eq!(s.mouse, Vec4::new(35.0, 45.0, -10.0, -20.0));
        assert!(!s.mouse_held);
        track_mouse(&mut s, at(50.0, 50.0), false, false);
        assert_eq!(s.mouse.x, 35.0, "hover alone moves nothing");
    }

    #[test]
    fn lucide_icons_resolve_in_proportional_fonts() {
        let ctx = egui::Context::default();
        ctx.set_fonts(lucide_fonts());
        // Fonts only exist after the first pass (pixels-per-point is set there).
        ctx.run_ui(egui::RawInput::default(), |_ui| {})
            .drop_without_applying_deltas();
        let font_id = egui::FontId::proportional(14.0);
        for icon in [
            lucide_icons::Icon::RotateCcw,
            lucide_icons::Icon::Bug,
            lucide_icons::Icon::Settings,
            lucide_icons::Icon::CircleX,
            lucide_icons::Icon::Check,
        ] {
            let ch = char::from(icon);
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&font_id, ch)),
                "lucide glyph {icon:?} (U+{:04X}) missing from the proportional fallback chain",
                ch as u32
            );
        }
    }
}
