//! Rendering wiring: the shadertoy fullscreen-material plugin.

pub mod buffers;
pub mod fullscreen_material;

use bevy::app::Plugin;
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::ecs::component::Component;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::math::Vec4;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_resource::ShaderType;
use bevy::render::RenderApp;

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
    /// Padding so `resolution` starts on a 16-byte boundary.
    pub _pad: f32,
    /// xy: viewport size in physical pixels, z: 1.0, w: unused.
    pub resolution: Vec4,
    /// Shadertoy-style mouse (see `i_mouse` in the environment module).
    pub mouse: Vec4,
    /// x: year, y: month (1-12), z: day of month, w: seconds since midnight.
    pub date: Vec4,
    /// Audio sample rate in Hz (44100; there is no audio input).
    pub sample_rate: f32,
    /// Padding so `channel_time` starts on a 16-byte boundary (the WGSL side
    /// spells this gap as three scalar pads, not a `vec3`, whose align-16
    /// rule would insert a different implicit gap).
    pub _pad2: f32,
    /// More padding.
    pub _pad3: f32,
    /// More padding.
    pub _pad4: f32,
    /// Playback time of buffer channels A..D (0.0 while a channel is unused).
    pub channel_time: Vec4,
    /// Size of buffer channels A..D in pixels (zero while a channel is unused).
    /// The WGSL side reads these as `vec3` (z = 0 for 2D buffers).
    pub channel_resolution: [Vec4; 4],
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
            ExtractResourcePlugin::<buffers::BufferMask>::default(),
        ));
        app.init_resource::<buffers::BufferMask>();

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<buffers::BufferMask>()
            .init_resource::<buffers::BufferTargets>()
            .init_resource::<buffers::BufferPipelines>()
            .add_systems(
                Core3d,
                buffers::shadertoy_buffer_pass
                    .in_set(Core3dSystems::PostProcess)
                    .before(fullscreen_material_system::<ShaderToyUniform>),
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
