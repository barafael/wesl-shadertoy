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
mod compile;
mod debugger;
mod editor;
#[cfg(not(target_arch = "wasm32"))]
mod headless;
mod highlight;
mod persist;
mod presets;
mod render;
mod theme;

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

use bevy::render::error_handler::{
    ErrorType, RenderError, RenderErrorHandler, RenderErrorPolicy,
};
use render::active::{ActiveShader, Playback, RejectedGeneration, ShaderFeedback};
use render::{ShaderToyFragment, ShaderToyRenderPlugin, ShaderToyUniform};
use theme::Theme;

/// Seconds of typing quiet before an auto-run compile fires (so a burst of
/// keystrokes compiles once, not once per character).
const AUTO_RUN_DELAY: f64 = 0.25;

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
        .init_resource::<RunRequest>()
        .init_resource::<UiWindows>()
        .init_resource::<editor::HighlightCache>()
        .init_resource::<analyzer::Analyzer>()
        .add_plugins(EguiPlugin::default())
        .add_plugins(ShaderToyRenderPlugin)
        .insert_resource(RenderErrorHandler(on_render_error))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                install_fonts,
                (update_uniforms, compile_and_apply, poll_render_feedback).chain(),
                run_analyzer,
                autosave,
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
    paused: bool,
    speed: f32,
    mouse: Vec4,
    mouse_held: bool,
    fps: f32,
    /// In-shader debugger overlay on/off (toolbar bug icon).
    debugger: bool,
    auto_run: bool,
    /// egui time of the last edit while auto-run is on; the `ui` system
    /// raises a run request once [`AUTO_RUN_DELAY`] seconds go by without a
    /// new edit (debounce, so typing does not compile per keystroke).
    pending_run: Option<f64>,
    last_preset: &'static str,
    /// The viewport area the panels leave free (egui points), recorded by
    /// [`ui`] each frame; iMouse only counts presses there.
    canvas: egui::Rect,
}

impl Default for ShaderToyState {
    fn default() -> Self {
        Self {
            time: 0.0,
            paused: false,
            speed: 1.0,
            mouse: Vec4::ZERO,
            mouse_held: false,
            fps: 0.0,
            debugger: false,
            auto_run: false,
            pending_run: None,
            last_preset: "Plasma",
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

/// Set by the Run button / Ctrl+Enter, consumed by [`compile_and_apply`].
#[derive(Resource, Default)]
struct RunRequest(bool);

#[derive(Resource)]
struct UiWindows {
    editor: bool,
    wgsl: bool,
    settings: bool,
    about: bool,
    errors_expanded: bool,
}

impl Default for UiWindows {
    fn default() -> Self {
        Self {
            editor: true,
            wgsl: false,
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
    theme: Theme,
    random_preset: bool,
    /// Live diagnostics from wgsl-analyzer while editing.
    analyzer: bool,
}

impl Settings {
    /// Used when nothing is saved yet: OS theme, random preset and live
    /// diagnostics on.
    fn fallback() -> Self {
        Self {
            theme: Theme::from_system(),
            random_preset: true,
            analyzer: true,
        }
    }

    fn encode(&self) -> String {
        format!(
            "theme={};random={};analyzer={}",
            self.theme.as_str(),
            self.random_preset as u8,
            self.analyzer as u8
        )
    }

    fn decode(s: &str) -> Self {
        let mut out = Self::fallback();
        for part in s.split(';') {
            match part.split_once('=') {
                Some(("theme", t)) => {
                    if let Some(theme) = Theme::parse(t) {
                        out.theme = theme;
                    }
                }
                Some(("random", r)) => out.random_preset = r == "1",
                Some(("analyzer", a)) => out.analyzer = a == "1",
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
}

fn setup(mut commands: Commands, mut shaders: ResMut<Assets<Shader>>, mut app: StartupParams) {
    let settings = persist::load_settings()
        .map(|s| Settings::decode(&s))
        .unwrap_or_else(Settings::fallback);
    let random = settings.random_preset;
    commands.insert_resource(settings);

    // `--shader <file>` (native) wins over everything: it is how scripted
    // live tests inject a document. Otherwise restore a saved document
    // (browser localStorage) or start from a preset — a random one when
    // the fresh-start setting is on.
    app.state.debugger = cli_debugger();
    if let Some(text) = cli_shader() {
        app.doc.text = text;
        app.state.last_preset = "file";
    } else {
        match persist::load_doc() {
            Some(saved) => {
                app.doc.text = saved;
                app.state.last_preset = "saved";
            }
            None => {
                let preset = cli_preset()
                    .or(random.then(random_preset))
                    .unwrap_or(presets::DEFAULT);
                app.doc.text = preset.code.to_string();
                app.state.last_preset = preset.name;
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
}

/// Advance the shadertoy clock, count frames, track iMouse and viewport size.
fn update_uniforms(mut p: UniformParams) {
    // Clamp so a backgrounded tab does not teleport the clock on resume.
    let dt = p.time.delta_secs().min(0.1);
    let step = if p.state.paused { 0.0 } else { dt * p.state.speed };
    p.state.time += step;
    if p.playback.paused != p.state.paused {
        p.playback.paused = p.state.paused;
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
    p.debugger_on.set_if_neq(debugger::DebuggerOn(p.state.debugger));
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
    let placeholder = main_world.resource::<CurrentShader>().placeholder_fragment();
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
/// preset picks and opened files alike (a no-op natively, where Ctrl+S
/// saves). Comparing against the last saved text is cheap next to a frame
/// and catches every path that changes the document.
fn autosave(doc: Res<ShaderDoc>, mut saved: Local<Option<String>>) {
    match saved.as_deref() {
        // The startup document (a preset or the restored save) is not an edit.
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
    let Ok(ctx) = p.contexts.ctx_mut() else {
        return;
    };
    theme::apply(ctx, p.settings.theme);

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
        p.state.pending_run = None;
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
    egui::Panel::top("toolbar").show(&mut root_ui, |ui| {
        ui.horizontal(|ui| {
            if ui
                .button("Compile WESL Shader")
                .on_hover_text("Compile & apply (Ctrl+Enter)")
                .clicked()
            {
                p.run.0 = true;
                p.state.pending_run = None;
            }
            let pause_label = if p.state.paused {
                icon(lucide_icons::Icon::Pause)
            } else {
                icon(lucide_icons::Icon::Play)
            };
            ui.toggle_value(&mut p.state.paused, pause_label)
                .on_hover_text("Pause / resume the clock");
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
            );
            // Fixed-width slots: digit changes must not reflow the toolbar.
            readout(ui, format!("t = {:.1}s", p.state.time), 84.0);
            readout(ui, format!("{:.0} fps", p.state.fps), 48.0);

            ui.separator();

            egui::ComboBox::from_id_salt("presets")
                .selected_text(format!("preset: {}", p.state.last_preset))
                .show_ui(ui, |ui| {
                    for preset in presets::ALL {
                        if ui
                            .selectable_label(p.state.last_preset == preset.name, preset.name)
                            .clicked()
                        {
                            p.doc.text = preset.code.to_string();
                            p.state.last_preset = preset.name;
                            p.state.pending_run = None;
                            p.run.0 = true;
                        }
                    }
                });

            ui.checkbox(&mut p.state.auto_run, "auto-run");

            ui.separator();
            ui.toggle_value(&mut p.wins.editor, "Code");
            ui.toggle_value(&mut p.wins.wgsl, "WGSL");
            ui.toggle_value(&mut p.state.debugger, icon(lucide_icons::Icon::Bug))
                .on_hover_text("In-shader debugger: crosshair, probe boxes, fps");
            ui.toggle_value(&mut p.wins.settings, icon(lucide_icons::Icon::Settings))
                .on_hover_text("Settings");
            ui.toggle_value(&mut p.wins.about, "?")
                .on_hover_text("About");
        });
        // Block canvas clicks through empty toolbar space.
        ui.allocate_rect(ui.max_rect(), egui::Sense::hover());
    });

    // --- status / error panel ----------------------------------------------
    egui::Panel::bottom("status").show(&mut root_ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(err) = &p.status.error {
                let summary = err.lines().next().unwrap_or("error").to_string();
                if ui
                    .selectable_label(
                        p.wins.errors_expanded,
                        format!("{} compile failed", icon(lucide_icons::Icon::CircleX)),
                    )
                    .on_hover_text("The previous shader keeps running")
                    .clicked()
                {
                    p.wins.errors_expanded = !p.wins.errors_expanded;
                }
                ui.label(egui::RichText::new(summary).color(p.settings.theme.err_color()));
            } else if let Some((note, ok)) = &p.status.note {
                let color = if *ok {
                    p.settings.theme.ok_color()
                } else {
                    p.settings.theme.err_color()
                };
                ui.label(egui::RichText::new(note.clone()).color(color));
            } else if let Some(ms) = p.status.last_ms {
                ui.label(
                    egui::RichText::new(format!(
                        "{} compiled in {ms:.1} ms",
                        icon(lucide_icons::Icon::Check)
                    ))
                    .color(p.settings.theme.ok_color()),
                );
            } else {
                ui.label("not compiled yet");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                analyzer_readout(ui, &p.analyzer, p.settings.theme);
            });
        });
        if p.wins.errors_expanded
            && let Some(err) = &p.status.error
        {
            ui.push_id("error_text", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut err.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
        }
        // Block canvas clicks through empty status-bar space.
        ui.allocate_rect(ui.max_rect(), egui::Sense::hover());
    });

    // --- canvas: what the panels leave free shows the shader ----------------
    p.state.canvas = root_ui.available_rect_before_wrap();

    // --- editor window -------------------------------------------------------
    if p.wins.editor {
        egui::Window::new("Code — package::user")
            .id(egui::Id::new("code_window"))
            .default_pos([12.0, 44.0])
            .default_size([640.0, 460.0])
            .open(&mut p.wins.editor)
            .show(ctx, |ui| {
                let theme = p.settings.theme;
                let out = editor::show_code_editor(
                    ui,
                    &mut p.doc.text,
                    theme,
                    &mut p.highlight,
                    p.analyzer.diagnostics(),
                );
                if out.changed && p.state.auto_run {
                    // Debounced: the compile fires AUTO_RUN_DELAY after the
                    // last keystroke, not once per character.
                    p.state.pending_run = Some(ctx.input(|i| i.time));
                }
            });
    }

    // --- debounced auto-run --------------------------------------------------
    if let Some(at) = p.state.pending_run
        && ctx.input(|i| i.time) - at >= AUTO_RUN_DELAY
    {
        p.state.pending_run = None;
        p.run.0 = true;
    }

    // --- generated WGSL viewer -----------------------------------------------
    if p.wins.wgsl {
        egui::Window::new("Generated WGSL — package::main")
            .id(egui::Id::new("wgsl_window"))
            .default_pos([680.0, 60.0])
            .default_size([520.0, 480.0])
            .open(&mut p.wins.wgsl)
            .show(ctx, |ui| {
                let theme = p.settings.theme;
                editor::show_code_view(ui, &p.current.wgsl, theme, &mut p.highlight);
            });
    }

    // --- settings window -----------------------------------------------------
    if p.wins.settings {
        egui::Window::new("Settings")
            .id(egui::Id::new("settings_window"))
            .default_pos([12.0, 60.0])
            .default_size([300.0, 150.0])
            .open(&mut p.wins.settings)
            .show(ctx, |ui| {
                let before = (
                    p.settings.theme,
                    p.settings.random_preset,
                    p.settings.analyzer,
                );
                ui.horizontal(|ui| {
                    ui.label("theme:");
                    ui.radio_value(&mut p.settings.theme, Theme::Dark, "gruvbox dark");
                    ui.radio_value(&mut p.settings.theme, Theme::Light, "gruvbox light");
                });
                ui.checkbox(
                    &mut p.settings.random_preset,
                    "random preset on fresh start",
                );
                ui.checkbox(&mut p.settings.analyzer, "live diagnostics (wgsl-analyzer)");
                if let analyzer::Status::Unavailable(why) = p.analyzer.status() {
                    ui.label(egui::RichText::new(why).small().weak());
                }
                ui.separator();
                ui.weak("Ctrl+S saves · Ctrl+O opens a .wesl file");
                let after = (
                    p.settings.theme,
                    p.settings.random_preset,
                    p.settings.analyzer,
                );
                if after != before {
                    persist::save_settings(&p.settings.encode());
                }
            });
    }

    // --- about window ----------------------------------------------------------
    if p.wins.about {
        egui::Window::new("About wesl-shadertoy")
            .id(egui::Id::new("about_window"))
            .default_pos([12.0, 60.0])
            .default_size([380.0, 330.0])
            .open(&mut p.wins.about)
            .show(ctx, |ui| {
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
                     · Gruvbox colors by morhetz · presets ported from shadertoy \
                     classics and public-domain snippets.",
                );
            });
    }
}

/// wgsl-analyzer's state at the right end of the status bar: problem counts
/// while it runs, otherwise why not (the details are in the hover text).
fn analyzer_readout(ui: &mut egui::Ui, analyzer: &analyzer::Analyzer, theme: Theme) {
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
                    (text, theme.err_color(), hover)
                }
                (Some(e), None) => (plural(e, "error"), theme.err_color(), hover),
                (None, Some(w)) => (plural(w, "warning"), ui.visuals().warn_fg_color, hover),
                (None, None) => (icon(lucide_icons::Icon::Check), theme.ok_color(), hover),
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
    state.last_preset = "file";
    state.pending_run = None;
    run.0 = true;
    status.note = Some((format!("opened {name}"), true));
}

/// Native-only: `--shader <file>` seeds the editor with the contents of a
/// WGSL file, so scripted live tests can inject a document without GUI
/// input. Takes precedence over saved documents and presets. Web builds
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
                        warn!("--shader {path}: {e}; starting with the default preset");
                        return None;
                    }
                }
            }
        }
        None
    }
}

/// Native-only: `--preset <name>` picks the startup preset (case-insensitive
/// match against [`presets::ALL`]), so scripted screenshot runs can pin a
/// deterministic shader. Web builds have no argv and always return `None`.
fn cli_preset() -> Option<&'static presets::Preset> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--preset"
                && let Some(name) = args.next()
            {
                return presets::ALL
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(&name));
            }
        }
        None
    }
}

/// Native-only: `--debugger` starts with the in-shader debugger overlay on
/// (same as the toolbar bug icon), so scripted live tests can show it.
fn cli_debugger() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::args().any(|a| a == "--debugger")
    }
}

/// Time-seeded xorshift pick — no `rand` dependency for one roll per start.
fn random_preset() -> &'static presets::Preset {
    let mut x = (now_epoch_secs() * 1.0e6) as u64;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let idx = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % presets::ALL.len();
    &presets::ALL[idx]
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
    Vec4::new(
        year as f32,
        month as f32,
        day as f32,
        seconds_of_day as f32,
    )
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
            lucide_icons::Icon::Play,
            lucide_icons::Icon::Pause,
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
