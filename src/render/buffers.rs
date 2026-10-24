//! Multi-pass buffer channels (shadertoy's Buffer A–D).
//!
//! When the editor shader defines `buffer_a`..`buffer_d` functions, the
//! compiler emits a `fragment_buffer_*` entry point for each (see
//! [`crate::compile`]) and this module renders them, in A→D order, into
//! offscreen `Rgba32Float` targets *before* the main fullscreen pass samples
//! them through the `i_channel_0..i_channel_3` globals.
//!
//! Each channel owns a ping-pong pair: a buffer reading its own channel sees
//! the previous frame (the texture it is *not* writing to right now), while a
//! buffer reading an earlier channel sees that channel's result from the
//! current frame. Channels whose buffer pass does not exist stay bound to a
//! black 1×1 dummy, matching shadertoy reporting them as unused.

use bevy::asset::AssetId;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut, SystemParam};
use bevy::math::UVec2;
use bevy::shader::Shader;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex};
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{
    BindGroupEntries, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d,
    FragmentState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureView, TextureViewDescriptor,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::RenderApp;

use super::fullscreen_material::{
    ChannelViews, FullscreenMaterialPipeline, ShaderToyFragment,
};
use super::ShaderToyUniform;

/// Which of `buffer_a`..`buffer_d` the current shader defines (bit per
/// [`crate::compile::BUFFER_NAMES`] entry). Lives in both worlds: the main
/// world writes it on every successful compile, the render world reads it in
/// [`shadertoy_buffer_pass`].
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
#[extract_app(RenderApp)]
pub struct BufferMask(pub u8);

/// The offscreen render target for one buffer channel slot.
struct BufferSurface {
    #[allow(dead_code)] // keeps the texture alive alongside its view
    texture: Texture,
    view: TextureView,
}

/// Ping-pong pair for one channel.
#[derive(Default)]
struct BufferPair {
    /// Slot written this frame; the other slot holds the previous frame.
    current: usize,
    surfaces: [Option<BufferSurface>; 2],
}

/// All four ping-pong pairs, (re)allocated at the current viewport size.
#[derive(Default, Resource)]
pub(crate) struct BufferTargets {
    size: UVec2,
    pairs: [BufferPair; 4],
}

impl BufferTargets {
    /// Drop contents when the viewport changed size; buffers always match it.
    fn resize(&mut self, size: UVec2) {
        let size = UVec2::new(size.x.max(1), size.y.max(1));
        if self.size != size {
            self.size = size;
            for pair in self.pairs.iter_mut() {
                pair.surfaces = [None, None];
                pair.current = 0;
            }
        }
    }

    /// The surface for channel `i`, slot `slot`, creating it on first use.
    fn surface(&mut self, i: usize, slot: usize, device: &RenderDevice) -> &BufferSurface {
        self.pairs[i].surfaces[slot].get_or_insert_with(|| {
            let texture = device.create_texture(&TextureDescriptor {
                label: Some(&format!(
                    "shadertoy_buffer_{}_{slot}",
                    crate::compile::BUFFER_NAMES[i]
                )),
                size: Extent3d {
                    width: self.size.x,
                    height: self.size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: BUFFER_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&TextureViewDescriptor::default());
            BufferSurface { texture, view }
        })
    }

    /// View this frame's pass for channel `i` writes into.
    fn write_view(&mut self, i: usize, device: &RenderDevice) -> TextureView {
        let current = self.pairs[i].current;
        self.surface(i, current, device).view.clone()
    }

    /// View of channel `i`'s previous frame (or, for a channel whose pass
    /// already ran this frame, of its current frame).
    fn read_view(&mut self, i: usize, device: &RenderDevice) -> TextureView {
        let slot = self.pairs[i].current ^ 1;
        self.surface(i, slot, device).view.clone()
    }
}

/// Buffers are float targets so feedback loops can accumulate HDR energy and
/// carry exact 32-bit payloads (some shaders pack `u32` into `f32` channels).
/// This matches shadertoy's default buffer format and the reference engine.
/// `Rgba32Float` is renderable but unfilterable, so channels are read with
/// `textureLoad` or sampled nearest through the non-filtering `i_sampler`
/// (see the template docs in [`crate::compile`]).
const BUFFER_FORMAT: TextureFormat = TextureFormat::Rgba32Float;

/// The four `fragment_buffer_*` pipelines, rebuilt when the shader handle or
/// buffer mask changes.
#[derive(Default, Resource)]
pub(crate) struct BufferPipelines {
    key: Option<(AssetId<Shader>, u8)>,
    ids: [Option<CachedRenderPipelineId>; 4],
}

type BufferPassView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ExtractedCamera,
        &'static DynamicUniformIndex<ShaderToyUniform>,
        Option<&'static ShaderToyFragment>,
    ),
>;

#[derive(SystemParam)]
pub struct BufferPassParams<'w> {
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: Res<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    data_uniforms: Res<'w, ComponentUniforms<ShaderToyUniform>>,
    render_device: Res<'w, RenderDevice>,
    mask: Res<'w, BufferMask>,
    channels: ResMut<'w, ChannelViews>,
    targets: ResMut<'w, BufferTargets>,
    pipelines: ResMut<'w, BufferPipelines>,
}

/// Render the defined buffer passes (A→D) into their ping-pong targets and
/// publish the results in [`ChannelViews`] for the main fullscreen pass.
///
/// Runs in `Core3d`'s post-process set, before
/// [`super::fullscreen_material::fullscreen_material_system`], recording into
/// the same command encoder so the passes land in order.
pub fn shadertoy_buffer_pass(
    views: BufferPassView<'_, '_>,
    mut p: BufferPassParams<'_>,
    mut ctx: RenderContext,
) {
    if p.mask.0 == 0 {
        p.channels.reset();
        return;
    }
    let view = views.into_inner();
    let Some(fragment) = view.2 else {
        return;
    };
    let Some(size) = view.0.physical_viewport_size else {
        return;
    };

    p.targets.resize(size);
    // Advance the clock: every pair swaps write/read slots once per frame.
    for pair in p.targets.pairs.iter_mut() {
        pair.current ^= 1;
    }

    // Channels without a pass read the black dummy.
    let dummy = p.channels.dummy.clone();
    for (i, slot) in p.channels.views.iter_mut().enumerate() {
        if p.mask.0 & (1 << i) == 0 {
            *slot = dummy.clone();
        }
    }

    rebuild_pipelines(&mut p.pipelines, &p.pipeline_cache, &p.fullscreen, fragment, p.mask.0);

    let Some(settings_binding) = p.data_uniforms.uniforms().binding() else {
        return;
    };
    let bgl = p.pipeline_cache.get_bind_group_layout(&p.fullscreen.layout);

    for (i, name) in crate::compile::BUFFER_NAMES.iter().enumerate() {
        if p.mask.0 & (1 << i) == 0 {
            continue;
        }
        let write = p.targets.write_view(i, &p.render_device);
        // Earlier buffers already wrote their current-frame slot; later and
        // self reads see the previous frame's slot.
        let reads: [TextureView; 4] = std::array::from_fn(|j| {
            if p.mask.0 & (1 << j) == 0 {
                dummy.clone()
            } else if j < i {
                p.targets.write_view(j, &p.render_device)
            } else {
                p.targets.read_view(j, &p.render_device)
            }
        });

        let Some(Some(pipeline)) = p.pipelines.ids[i].map(|id| p.pipeline_cache.get_render_pipeline(id))
        else {
            continue; // pipeline still compiling; channel keeps its stale view
        };

        let bind_group = p.render_device.create_bind_group(
            Some(format!("shadertoy_buffer_bind_group_{name}").as_str()),
            &bgl,
            &BindGroupEntries::sequential((
                &dummy,
                &p.fullscreen.sampler,
                settings_binding.clone(),
                &reads[0],
                &reads[1],
                &reads[2],
                &reads[3],
            )),
        );

        let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
            label: Some(&format!("shadertoy_buffer_pass_{name}")),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &write,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[view.1.index()]);
        pass.draw(0..3, 0..1);
        drop(pass);

        p.channels.views[i] = write;
    }
}

/// Queue the `fragment_buffer_*` pipelines for `mask`, unless the current
/// shader + mask combination already has them.
fn rebuild_pipelines(
    pipelines: &mut ResMut<'_, BufferPipelines>,
    pipeline_cache: &PipelineCache,
    fullscreen: &FullscreenMaterialPipeline<ShaderToyUniform>,
    fragment: &ShaderToyFragment,
    mask: u8,
) {
    let key = (fragment.0.id(), mask);
    if pipelines.key == Some(key) {
        return;
    }
    pipelines.key = Some(key);
    for (i, name) in crate::compile::BUFFER_NAMES.iter().enumerate() {
        pipelines.ids[i] = None;
        if mask & (1 << i) == 0 {
            continue;
        }
        let descriptor = RenderPipelineDescriptor {
            label: Some(format!("shadertoy_buffer_pipeline_{name}").into()),
            layout: vec![fullscreen.layout.clone()],
            vertex: fullscreen.vertex.clone(),
            fragment: Some(FragmentState {
                shader: fragment.0.clone(),
                entry_point: Some(crate::compile::buffer_entry(name).into()),
                targets: vec![Some(ColorTargetState {
                    format: BUFFER_FORMAT,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            ..Default::default()
        };
        pipelines.ids[i] = Some(pipeline_cache.queue_render_pipeline(descriptor));
    }
}
