//! Rendering wiring: the shadertoy fullscreen-material plugin.

pub mod fullscreen_material;

use bevy::ecs::component::Component;
use bevy::math::Vec4;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::render::RenderApp;

pub use fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin, ShaderToyFragment};

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
    /// x: seconds since midnight, y: month, z: day of week (0 = Sunday), w: day of year.
    pub date: Vec4,
}

impl FullscreenMaterial for ShaderToyUniform {
    // Runs in Core3d post-process; with an empty scene the fullscreen triangle
    // simply shades the cleared view target.
}

/// Plugin that renders the user's compiled shader full-screen.
pub struct ShaderToyRenderPlugin;

impl bevy::app::Plugin for ShaderToyRenderPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_plugins(FullscreenMaterialPlugin::<ShaderToyUniform>::default());
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
