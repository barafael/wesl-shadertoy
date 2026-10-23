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

mod compile;
mod editor;
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

use render::{ShaderToyFragment, ShaderToyRenderPlugin, ShaderToyUniform};
use theme::Theme;

fn main() {
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
        .add_plugins(EguiPlugin::default())
        .add_plugins(ShaderToyRenderPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (update_uniforms, compile_and_apply).chain())
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
    frame: u32,
    mouse: Vec4,
    mouse_held: bool,
    fps: f32,
    auto_run: bool,
    last_preset: &'static str,
}

impl Default for ShaderToyState {
    fn default() -> Self {
        Self {
            time: 0.0,
            paused: false,
            speed: 1.0,
            frame: 0,
            mouse: Vec4::ZERO,
            mouse_held: false,
            fps: 0.0,
            auto_run: false,
            last_preset: "Plasma",
        }
    }
}

/// The shader currently running on screen.
#[derive(Resource, Default)]
struct CurrentShader {
    handle: Handle<Shader>,
    wgsl: String,
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
}

impl Settings {
    /// Used when nothing is saved yet: OS theme, random preset on.
    fn fallback() -> Self {
        Self {
            theme: Theme::from_system(),
            random_preset: true,
        }
    }

    fn encode(&self) -> String {
        format!(
            "theme={};random={}",
            self.theme.as_str(),
            self.random_preset as u8
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

    // Restore a saved document (browser localStorage) or start from a preset
    // — a random one when the fresh-start setting is on.
    match persist::load_doc() {
        Some(saved) => {
            app.doc.text = saved;
            app.state.last_preset = "saved";
        }
        None => {
            let preset = if random {
                random_preset()
            } else {
                presets::DEFAULT
            };
            app.doc.text = preset.code.to_string();
            app.state.last_preset = preset.name;
        }
    }

    // A black placeholder so the pipeline has *something* until the first
    // successful compile lands later this frame.
    let placeholder = Shader::from_wgsl(render::placeholder_wgsl(), "shadertoy://placeholder");
    let handle = shaders.add(placeholder);
    app.current.handle = handle.clone();

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
        ShaderToyFragment(handle),
    ));

    // Compile whatever is in the editor on the first frame.
    app.run.0 = true;
}

/// Everything [`update_uniforms`] reads or writes, bundled into one parameter.
#[derive(SystemParam)]
struct UniformParams<'w, 's> {
    contexts: EguiContexts<'w, 's>,
    state: ResMut<'w, ShaderToyState>,
    time: Res<'w, Time>,
    windows: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    uniforms: Single<'w, 's, &'static mut ShaderToyUniform>,
}

/// Advance the shadertoy clock, count frames, track iMouse and viewport size.
fn update_uniforms(mut p: UniformParams) {
    // Clamp so a backgrounded tab does not teleport the clock on resume.
    let dt = p.time.delta_secs().min(0.1);
    if !p.state.paused {
        p.state.time += dt * p.state.speed;
    }
    p.state.frame += 1;
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
    // WGSL `@builtin(position)` has its origin top-left, same as egui, so no
    // y-flip is needed. Clicks that start on an egui widget are ignored.
    if let Ok(ctx) = p.contexts.ctx_mut() {
        let ppp = ctx.pixels_per_point();
        let (hover, pressed, released, held) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });
        // A pointer over an egui window/panel "interacts" with that area's
        // background widget, so `interact_pos` is Some; over the raw canvas
        // (which is not an egui widget) it is None.
        let over_ui = ctx.input(|i| i.pointer.interact_pos().is_some());

        if let Some(pos) = hover {
            let px = pos * ppp;
            if pressed && !over_ui {
                p.state.mouse = Vec4::new(px.x, px.y, px.x, px.y);
                p.state.mouse_held = true;
            } else if held && p.state.mouse_held {
                p.state.mouse = Vec4::new(px.x, px.y, p.state.mouse.z, p.state.mouse.w);
            }
            if released && p.state.mouse_held {
                p.state.mouse_held = false;
                p.state.mouse = Vec4::new(px.x, px.y, -p.state.mouse.z, -p.state.mouse.w);
            }
        }
    }

    p.uniforms.time = p.state.time;
    p.uniforms.time_delta = if p.state.paused { 0.0 } else { dt };
    p.uniforms.frame = p.state.frame;
    p.uniforms.resolution = resolution;
    p.uniforms.mouse = p.state.mouse;
    p.uniforms.date = date_uniforms();
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
        Ok(wgsl) => {
            let shader = Shader::from_wgsl(wgsl.clone(), "shadertoy://user");
            let handle = p.shaders.add(shader);
            p.fragment.0 = handle.clone();
            p.current.handle = handle;
            p.current.wgsl = wgsl;
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
            }
            let pause_label = if p.state.paused { "⏸" } else { "▶︎" };
            ui.toggle_value(&mut p.state.paused, pause_label)
                .on_hover_text("Pause / resume the clock");
            if ui.button("⟲").on_hover_text("Reset time").clicked() {
                p.state.time = 0.0;
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
                            p.run.0 = true;
                        }
                    }
                });

            ui.checkbox(&mut p.state.auto_run, "auto-run");

            ui.separator();
            ui.toggle_value(&mut p.wins.editor, "Code");
            ui.toggle_value(&mut p.wins.wgsl, "WGSL");
            ui.toggle_value(&mut p.wins.settings, "⚙")
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
                    .selectable_label(p.wins.errors_expanded, "✗ compile failed")
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
                    egui::RichText::new(format!("✓ compiled in {ms:.1} ms"))
                        .color(p.settings.theme.ok_color()),
                );
            } else {
                ui.label("not compiled yet");
            }
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

    // --- editor window -------------------------------------------------------
    if p.wins.editor {
        egui::Window::new("Code — package::user")
            .id(egui::Id::new("code_window"))
            .default_pos([12.0, 44.0])
            .default_size([640.0, 460.0])
            .open(&mut p.wins.editor)
            .show(ctx, |ui| {
                let syntax = p.settings.theme.syntax();
                let out = editor::show_code_editor(ui, &mut p.doc.text, &syntax);
                if out.changed {
                    persist::save_dirty(ctx, &p.doc.text);
                    if p.state.auto_run {
                        p.run.0 = true;
                    }
                }
            });
    }

    // --- generated WGSL viewer -----------------------------------------------
    if p.wins.wgsl {
        egui::Window::new("Generated WGSL — package::main")
            .id(egui::Id::new("wgsl_window"))
            .default_pos([680.0, 60.0])
            .default_size([520.0, 480.0])
            .open(&mut p.wins.wgsl)
            .show(ctx, |ui| {
                let syntax = p.settings.theme.syntax();
                editor::show_code_view(ui, &p.current.wgsl, &syntax);
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
                let before = (p.settings.theme, p.settings.random_preset);
                ui.horizontal(|ui| {
                    ui.label("theme:");
                    ui.radio_value(&mut p.settings.theme, Theme::Dark, "gruvbox dark");
                    ui.radio_value(&mut p.settings.theme, Theme::Light, "gruvbox light");
                });
                ui.checkbox(
                    &mut p.settings.random_preset,
                    "random preset on fresh start",
                );
                ui.separator();
                ui.weak("Ctrl+S saves · Ctrl+O opens a .wesl file");
                if (p.settings.theme, p.settings.random_preset) != before {
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
                     · Gruvbox colors by morhetz · presets ported from shadertoy \
                     classics and public-domain snippets.",
                );
            });
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
    run.0 = true;
    status.note = Some((format!("opened {name}"), true));
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

/// x: seconds since midnight, y: month, z: day of week (0 = Sunday),
/// w: day of year. (Matches the `i_date` docs in the environment module.)
fn date_uniforms() -> Vec4 {
    let secs = now_epoch_secs();
    let days = (secs / 86400.0).floor() as i64;
    let seconds_of_day = secs - days as f64 * 86400.0;
    let (year, month, day) = civil_from_days(days);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_of_year = CUM[(month as usize - 1).min(11)] + day;
    if leap && month > 2 {
        day_of_year += 1;
    }
    let dow = (days.rem_euclid(7) + 4) as f32 % 7.0; // 1970-01-01 was a Thursday
    Vec4::new(seconds_of_day as f32, month as f32, dow, day_of_year as f32)
}
