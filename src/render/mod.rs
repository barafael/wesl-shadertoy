//! Rendering wiring: the shadertoy fullscreen-material plugin.

pub mod active;
pub mod blur;
pub mod buffers;
pub mod fullscreen_material;
pub mod previews;

use bevy::app::Plugin;
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::ecs::component::Component;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::math::Vec4;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_resource::ShaderType;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

pub use fullscreen_material::ShaderToyFragment;
use fullscreen_material::{
    FullscreenMaterial, FullscreenMaterialPlugin, fullscreen_material_system,
};

/// Per-frame shadertoy uniforms, extracted to the render world as a dynamic
/// uniform buffer bound at `@group(0) @binding(2)`.
///
/// Field order must match `ShaderToyUniforms` in [`crate::compile::SHADERTOY_MODULE`].
/// All members are 16-byte aligned so the layout is also valid for WebGL2.
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default, Debug)]
#[extract_app(RenderApp)]
pub struct ShaderToyUniform {
    /// Seconds since start (or since the clock was reset), frozen while paused.
    pub time: f32,
    /// Seconds since the previous frame (0 while paused).
    pub time_delta: f32,
    /// Render frame counter.
    pub frame: u32,
    /// Heat mode only: the loop count at the hot end of the heatmap's
    /// palette (`crate::heatmap`). Also pads `resolution` onto a 16-byte
    /// boundary.
    pub heat_max: f32,
    /// xy: viewport size in physical pixels, z: 1.0, w: unused.
    pub resolution: Vec4,
    /// Shadertoy-style mouse (see `i_mouse` in the environment module).
    pub mouse: Vec4,
    /// x: year, y: month (1-12), z: day of month, w: seconds since midnight.
    pub date: Vec4,
    /// Audio sample rate in Hz (44100; there is no audio input).
    pub sample_rate: f32,
    /// Smoothed frames per second (`i_frame_rate()`, also shown by the
    /// debugger overlay).
    pub frame_rate: f32,
    /// Debugger overlay only: physical-pixel rows at the top and bottom of
    /// the viewport hidden under the toolbar and the status bar, so the
    /// overlay's readouts stay visible. Not exposed to user shaders.
    pub overlay_top: f32,
    /// See [`Self::overlay_top`].
    pub overlay_bottom: f32,
    /// Playback time of buffer channels A..D (0.0 while a channel is unused).
    pub channel_time: Vec4,
    /// Size of buffer channels A..D in pixels (zero while a channel is unused).
    /// The WGSL side reads these as `vec3` (z = 0 for 2D buffers).
    pub channel_resolution: [Vec4; 4],
    /// Live values of the Constants window's tweakable constants, one slot
    /// each (`crate::tweak`); read through the generated loader.
    pub tweaks: [Vec4; crate::tweak::MAX_TWEAKS],
}

/// `uniform`'s bytes as the GPU sees them (std140 / WGSL uniform layout).
/// The headless renderer uploads these, so its layout always follows the
/// struct instead of hand-written offsets.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn uniform_bytes(uniform: &ShaderToyUniform) -> Vec<u8> {
    let mut buffer = bevy::render::render_resource::encase::UniformBuffer::new(Vec::new());
    buffer
        .write(uniform)
        .expect("writing into a Vec cannot fail");
    buffer.into_inner()
}

impl FullscreenMaterial for ShaderToyUniform {
    // Runs in Core3d post-process; with an empty scene the fullscreen triangle
    // simply shades the cleared view target.

    /// The user's `main_image` entry; the `fragment_buffer_*` siblings are
    /// rendered by [`super::buffers::shadertoy_buffer_pass`] instead.
    fn fragment_entry_point() -> Option<&'static str> {
        Some(crate::compile::MAIN_ENTRY)
    }
}

/// Plugin that renders the user's compiled shader full-screen.
pub struct ShaderToyRenderPlugin;

impl Plugin for ShaderToyRenderPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_plugins((
            FullscreenMaterialPlugin::<ShaderToyUniform>::default(),
            ExtractResourcePlugin::<active::Playback>::default(),
            ExtractResourcePlugin::<crate::debugger::DebuggerShader>::default(),
            ExtractResourcePlugin::<crate::debugger::DebuggerOn>::default(),
            ExtractResourcePlugin::<blur::Blur>::default(),
            ExtractResourcePlugin::<previews::PreviewJobs>::default(),
        ));
        // Feedback channels, shared (not extracted) by both worlds.
        let feedback = active::ShaderFeedback::default();
        let preview_feedback = previews::PreviewFeedback::default();
        app.init_resource::<active::Playback>()
            .init_resource::<crate::debugger::DebuggerOn>()
            .init_resource::<previews::PreviewJobs>()
            .insert_resource(feedback.clone())
            .insert_resource(preview_feedback.clone());
        app.add_systems(
            bevy::app::Startup,
            (crate::debugger::setup_debugger_shader, blur::setup_blur),
        )
        .add_systems(bevy::app::PostUpdate, blur::size_blur_image);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<active::Playback>()
            .init_resource::<active::RejectedGeneration>()
            .insert_resource(feedback)
            .insert_resource(preview_feedback)
            .init_resource::<previews::PreviewState>()
            .init_resource::<buffers::BufferTargets>()
            .init_resource::<buffers::BufferPipelines>()
            .init_resource::<crate::debugger::DebuggerOn>()
            .add_systems(
                RenderStartup,
                (crate::debugger::setup_debugger, blur::setup_blur_pipelines),
            )
            .add_systems(Render, active::activate_shader.in_set(RenderSystems::Queue))
            .add_systems(
                Core3d,
                (
                    crate::debugger::resize_image_target
                        .in_set(Core3dSystems::PostProcess)
                        .before(fullscreen_material_system::<ShaderToyUniform>),
                    crate::debugger::debugger_blit_pass
                        .in_set(Core3dSystems::PostProcess)
                        .after(fullscreen_material_system::<ShaderToyUniform>),
                    blur::blur_pass
                        .in_set(Core3dSystems::PostProcess)
                        .after(crate::debugger::debugger_blit_pass),
                ),
            )
            .add_systems(
                Core3d,
                (
                    previews::preview_pass
                        .in_set(Core3dSystems::PostProcess)
                        .before(buffers::shadertoy_buffer_pass),
                    buffers::shadertoy_buffer_pass
                        .in_set(Core3dSystems::PostProcess)
                        .before(fullscreen_material_system::<ShaderToyUniform>),
                ),
            );
    }
}

/// The WGSL source used until the first successful compile (a black screen).
pub fn placeholder_wgsl() -> String {
    r#"// placeholder installed by wesl-shadertoy until the first Run
struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#
    .to_string()
}
