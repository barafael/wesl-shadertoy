//! The shader eats the editor: an opt-in mode in which the user shader gets
//! the rendered UI as a texture, `i_ui`, and its output is what reaches the
//! screen, so it can ripple, smear or melt the editor you are typing into.
//!
//! The UI is still egui's primary context, laid out and hit-tested as
//! always; only its pixels take the detour through the shader
//! ([`crate::render::ui_pass`]). Typing and clicking therefore land where
//! the undistorted UI would be. A shader that never reads `i_ui` would hide
//! the UI altogether, so the toolbar toggle only works for shaders that do,
//! and [`HOTKEY`] always switches the mode off. It is deliberately not saved
//! with the settings: a reload always starts with a readable UI.
//!
//! The frosted glass and the debugger overlay sit this mode out: both read
//! the shader's image before egui draws, which now comes after.

use bevy::app::{App, Plugin, Startup, Update};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut};
use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::render::RenderApp;
use bevy::render::extract_resource::ExtractResource;
use bevy_egui::egui;

/// The binding `i_ui` sits at in `package::shadertoy` (its sampler follows).
pub const UI_BINDING: u32 = 7;

/// Switches the mode off, whatever has focus. F2 has no browser or desktop
/// meaning, and none in the editor.
pub const HOTKEY: KeyCode = KeyCode::F2;

/// Whether the UI goes through the shader. Toggled from the toolbar,
/// extracted to the render world for its passes.
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
#[extract_app(RenderApp)]
pub struct UiTexture {
    pub on: bool,
}

/// The main-world half: the emergency [`HOTKEY`] and `--ui-texture`. (The
/// resource and the render passes are registered by
/// [`crate::render::ShaderToyRenderPlugin`].)
pub struct UiTexturePlugin;

impl Plugin for UiTexturePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, start_on)
            .add_systems(Update, emergency_off);
    }
}

/// Native-only: `--ui-texture` starts with the mode on, so scripted
/// screenshot runs can show it (with a shader that reads `i_ui`, e.g.
/// `--example "UI Melt"`).
fn start_on(mut ui_texture: ResMut<UiTexture>) {
    if crate::cli_flag("--ui-texture") {
        ui_texture.on = true;
    }
}

/// [`HOTKEY`] switches the mode off. Reads Bevy's keyboard state rather
/// than egui's, so it works however mangled the UI on screen is.
fn emergency_off(keys: Res<ButtonInput<KeyCode>>, mut ui_texture: ResMut<UiTexture>) {
    if keys.just_pressed(HOTKEY) && ui_texture.on {
        ui_texture.on = false;
    }
}

/// Whether a compiled module reads the UI texture: `i_ui` is stripped from
/// the output unless something uses it.
pub fn uses_ui(wgsl: &str) -> bool {
    wgsl.contains(&format!("@binding({UI_BINDING})"))
}

/// The toolbar toggle. `wgsl` is the latest compile: a shader that does not
/// read `i_ui` cannot switch the mode on, and switches it off.
pub fn toolbar_toggle(ui: &mut egui::Ui, ui_texture: &mut UiTexture, wgsl: &str) {
    let usable = uses_ui(wgsl);
    if !usable && ui_texture.on {
        ui_texture.on = false;
    }
    let mut on = ui_texture.on;
    ui.add_enabled_ui(usable, |ui| {
        ui.toggle_value(&mut on, crate::icon(lucide_icons::Icon::Droplets))
            .on_hover_text(
                "Feed the UI to the shader as i_ui (try the UI Melt example). \
                 F2 switches it off.",
            )
            .on_disabled_hover_text(
                "Feeds the UI to the shader as i_ui; this shader does not read it \
                 (try the UI Melt example)",
            );
    });
    if on != ui_texture.on {
        ui_texture.on = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_user_shader;

    /// Only shaders that read the UI enable the mode; `i_ui` is stripped
    /// from everything else.
    #[test]
    fn detects_ui_readers() {
        let reads = r#"
import package::shadertoy::{i_resolution, i_ui_sample};
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return i_ui_sample(frag_coord / i_resolution().xy);
}
"#;
        let ignores = r#"
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(frag_coord.xyx * 0.001, 1.0);
}
"#;
        assert!(uses_ui(&compile_user_shader(reads).unwrap().wgsl));
        assert!(!uses_ui(&compile_user_shader(ignores).unwrap().wgsl));
        assert!(!uses_ui(""), "nothing compiled yet");
        let melt = crate::examples::by_name("UI Melt").expect("UI Melt example");
        assert!(uses_ui(&compile_user_shader(melt.code).unwrap().wgsl));
        let plasma = crate::examples::by_name("Plasma").unwrap();
        assert!(!uses_ui(&compile_user_shader(plasma.code).unwrap().wgsl));
    }

    /// The raw texture is importable too, for `textureLoad`.
    #[test]
    fn i_ui_texture_is_importable() {
        let src = r#"
import package::shadertoy::{i_ui, i_ui_sampler};
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let texel = textureLoad(i_ui, vec2<i32>(frag_coord), 0);
    return texel + textureSampleLevel(i_ui, i_ui_sampler, vec2<f32>(0.5), 0.0);
}
"#;
        let wgsl = compile_user_shader(src).expect("compiles").wgsl;
        assert!(uses_ui(&wgsl));
        assert!(wgsl.contains(&format!("@binding({})", UI_BINDING + 1)));
    }

    /// A shader that stops reading `i_ui` switches the mode off, and cannot
    /// switch it on.
    #[test]
    fn toggle_follows_the_shader() {
        let ctx = egui::Context::default();
        let mut state = UiTexture { on: true };
        let wgsl = format!("@group(0) @binding({UI_BINDING}) var i_ui: texture_2d<f32>;");
        let frame = |state: &mut UiTexture, wgsl: &str| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                toolbar_toggle(ui, state, wgsl);
            })
            .drop_without_applying_deltas();
        };
        frame(&mut state, &wgsl);
        assert!(state.on, "a UI reader keeps it on");
        frame(&mut state, "");
        assert!(!state.on, "a shader without i_ui switches it off");
    }

    /// The hotkey switches the mode off and leaves it off.
    #[test]
    fn hotkey_switches_off() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(UiTexture { on: true })
            .add_plugins(UiTexturePlugin);
        app.update();
        assert!(app.world().resource::<UiTexture>().on);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(HOTKEY);
        app.update();
        assert!(!app.world().resource::<UiTexture>().on);
    }

    /// The toggle's icon resolves in the installed fonts.
    #[test]
    fn toggle_icon_resolves() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::lucide_fonts());
        ctx.run_ui(egui::RawInput::default(), |_ui| {})
            .drop_without_applying_deltas();
        let ch = char::from(lucide_icons::Icon::Droplets);
        assert!(ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), ch)));
    }
}
