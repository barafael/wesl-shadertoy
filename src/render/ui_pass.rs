//! The render side of the UI texture mode ([`crate::ui_texture`]): the user
//! shader runs after egui, reading the rendered UI as `i_ui`, and its output
//! is what reaches the screen.
//!
//! While the mode is on, the frame goes: the buffer passes as usual (the
//! main pass in [`super::fullscreen_material`] stands aside), then
//! [`ui_clear_pass`] clears the view to transparent, egui draws the UI into
//! it (premultiplied alpha, so the UI's coverage lands in alpha), and
//! [`ui_melt_pass`] runs the user shader with that image bound as `i_ui`,
//! writing the view's other main texture, which upscaling then presents.
//! egui keeps its own input and layout, so the UI stays usable where it
//! would be undistorted.
//!
//! While the mode is off, `i_ui` is a transparent 1×1 texture and neither
//! pass records anything.

use bevy::ecs::system::Res;
use bevy::render::extract_component::DynamicUniformIndex;
use bevy::render::render_resource::{
    AddressMode, Extent3d, FilterMode, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDescriptor, Sampler, SamplerDescriptor, StoreOp, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::ViewTarget;

use super::ShaderToyUniform;
use super::fullscreen_material::{FullscreenMaterialPipelineId, MainPassParams};
use crate::ui_texture::UiTexture;

/// The transparent 1×1 stand-in for `i_ui` and the bilinear, clamp-to-edge
/// `i_ui_sampler`, for the fullscreen-material bind group layout.
pub fn ui_bindings(device: &RenderDevice) -> (TextureView, Sampler) {
    // Textures read as zero before anything is written: transparent black.
    // Rgba8UnormSrgb like the view target, so the layout can call it
    // filterable.
    let dummy = device.create_texture(&TextureDescriptor {
        label: Some("shadertoy_ui_dummy"),
        size: Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("shadertoy_ui_sampler"),
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..Default::default()
    });
    (
        dummy.create_view(&TextureViewDescriptor::default()),
        sampler,
    )
}

type UiView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ViewTarget,
        &'static DynamicUniformIndex<ShaderToyUniform>,
        // Only views the fullscreen-material plugin drives.
        &'static FullscreenMaterialPipelineId<ShaderToyUniform>,
    ),
>;

/// Clear the view to transparent for egui to draw on, so `i_ui` holds the
/// UI alone. Runs at the end of the post-process set, after everything that
/// writes the view there (the debugger blit, which draws nothing useful in
/// this mode) and reads it (the blur, off in this mode).
pub fn ui_clear_pass(view: UiView<'_, '_>, ui_texture: Res<UiTexture>, mut ctx: RenderContext) {
    if !ui_texture.on {
        return;
    }
    let (view_target, _, _) = view.into_inner();
    ctx.command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("ui_clear_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: view_target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Default::default()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
}

/// The user shader's image pass with the UI egui just drew bound as `i_ui`,
/// written to the view. Runs after egui and before upscaling. While the
/// pipeline is still compiling the UI stays on screen undistorted.
pub fn ui_melt_pass(
    view: UiView<'_, '_>,
    p: MainPassParams<ShaderToyUniform>,
    ui_texture: Res<UiTexture>,
    mut ctx: RenderContext,
) {
    if !ui_texture.on {
        return;
    }
    let (view_target, settings_index, pipeline_id) = view.into_inner();
    if !p.ready(pipeline_id.0) {
        return;
    }
    let post_process = view_target.post_process_write();
    p.draw(
        &mut ctx,
        "ui_melt_pass",
        pipeline_id.0,
        settings_index.index(),
        post_process.source,
        Some(post_process.source),
        post_process.destination,
    );
}
