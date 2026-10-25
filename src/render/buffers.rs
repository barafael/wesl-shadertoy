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
//!
//! The passes follow the shader [`super::active`] draws, not the one last
//! requested. The targets are cleared when that shader is activated, when
//! the Reset button bumps [`Playback::epoch`] and on resize; while paused
//! the passes hold still and the channels keep showing their last result.

use bevy::asset::{AssetId, Handle};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut, SystemParam};
use bevy::math::UVec2;
use bevy::platform::collections::HashMap;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex};
use bevy::render::render_resource::{
    BindGroupEntries, BindingResource, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    CommandEncoder, Extent3d, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Texture, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::shader::Shader;

use super::ShaderToyUniform;
use super::active::{ActiveShader, Playback};
use super::fullscreen_material::{ChannelViews, FullscreenMaterialPipeline};

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
    /// `(generation, epoch)` of the activation the contents belong to.
    owner: Option<(u32, u32)>,
    pairs: [BufferPair; 4],
}

impl BufferTargets {
    /// Drop contents when the viewport changed size (buffers always match
    /// it) or a different activation took over. Returns whether it did;
    /// fresh textures read as zero.
    pub(crate) fn reset_if_stale(&mut self, size: UVec2, owner: (u32, u32)) -> bool {
        let size = size.max(UVec2::ONE);
        if self.size == size && self.owner == Some(owner) {
            return false;
        }
        self.size = size;
        self.owner = Some(owner);
        for pair in self.pairs.iter_mut() {
            pair.surfaces = [None, None];
            pair.current = 0;
        }
        true
    }

    /// Start a frame: every pair swaps its write and read slots.
    pub(crate) fn advance(&mut self) {
        for pair in self.pairs.iter_mut() {
            pair.current ^= 1;
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

/// The `fragment_buffer_*` pipelines, one per shader handle and channel.
/// The app reuses a fixed set of shader handles, so this stays small, and a
/// replaced shader asset rebuilds its pipelines in place.
#[derive(Default, Resource)]
pub(crate) struct BufferPipelines {
    ids: HashMap<(AssetId<Shader>, usize), CachedRenderPipelineId>,
}

impl BufferPipelines {
    /// The pipelines for the channels in `mask`, queued on first request.
    pub fn ids_for(
        &mut self,
        pipeline_cache: &PipelineCache,
        fullscreen: &FullscreenMaterialPipeline<ShaderToyUniform>,
        shader: &Handle<Shader>,
        mask: u8,
    ) -> [Option<CachedRenderPipelineId>; 4] {
        std::array::from_fn(|i| {
            if mask & (1 << i) == 0 {
                return None;
            }
            let id = *self.ids.entry((shader.id(), i)).or_insert_with(|| {
                let name = crate::compile::BUFFER_NAMES[i];
                pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(format!("shadertoy_buffer_pipeline_{name}").into()),
                    layout: vec![fullscreen.layout.clone()],
                    vertex: fullscreen.vertex.clone(),
                    fragment: Some(FragmentState {
                        shader: shader.clone(),
                        entry_point: Some(crate::compile::buffer_entry(name).into()),
                        targets: vec![Some(ColorTargetState {
                            format: BUFFER_FORMAT,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            });
            Some(id)
        })
    }
}

/// One frame's buffer passes for one shader: what [`Self::draw`] binds.
pub(crate) struct BufferPass<'a> {
    pub device: &'a RenderDevice,
    pub pipeline_cache: &'a PipelineCache,
    pub fullscreen: &'a FullscreenMaterialPipeline<ShaderToyUniform>,
    /// From [`BufferPipelines::ids_for`].
    pub ids: [Option<CachedRenderPipelineId>; 4],
    /// Bit per channel the shader defines.
    pub mask: u8,
    /// The shader's uniforms: a dynamic uniform buffer and its offset.
    pub uniforms: BindingResource<'a>,
    pub offset: u32,
    /// [`ChannelViews::dummy`], bound for channels the shader lacks.
    pub dummy: &'a TextureView,
}

impl BufferPass<'_> {
    /// Render the defined buffer passes (A→D) into `targets` (already
    /// [advanced](BufferTargets::advance) for this frame), publishing each
    /// result in `channels` for the passes after it and the image pass.
    /// Undefined channels are pointed at the black dummy; a channel whose
    /// pipeline is still compiling keeps its previous view.
    pub fn draw(
        &self,
        encoder: &mut CommandEncoder,
        targets: &mut BufferTargets,
        channels: &mut [TextureView; 4],
    ) {
        let dummy = self.dummy;
        for (i, slot) in channels.iter_mut().enumerate() {
            if self.mask & (1 << i) == 0 {
                *slot = dummy.clone();
            }
        }
        let bgl = self
            .pipeline_cache
            .get_bind_group_layout(&self.fullscreen.layout);
        for (i, name) in crate::compile::BUFFER_NAMES.iter().enumerate() {
            if self.mask & (1 << i) == 0 {
                continue;
            }
            let write = targets.write_view(i, self.device);
            // Earlier buffers already wrote their current-frame slot; later
            // and self reads see the previous frame's slot.
            let reads: [TextureView; 4] = std::array::from_fn(|j| {
                if self.mask & (1 << j) == 0 {
                    dummy.clone()
                } else if j < i {
                    targets.write_view(j, self.device)
                } else {
                    targets.read_view(j, self.device)
                }
            });

            let Some(Some(pipeline)) =
                self.ids[i].map(|id| self.pipeline_cache.get_render_pipeline(id))
            else {
                continue; // pipeline still compiling; channel keeps its stale view
            };

            let bind_group = self.device.create_bind_group(
                Some(format!("shadertoy_buffer_bind_group_{name}").as_str()),
                &bgl,
                &BindGroupEntries::sequential((
                    dummy,
                    &self.fullscreen.sampler,
                    self.uniforms.clone(),
                    &reads[0],
                    &reads[1],
                    &reads[2],
                    &reads[3],
                )),
            );

            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
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
            pass.set_bind_group(0, &bind_group, &[self.offset]);
            pass.draw(0..3, 0..1);
            drop(pass);

            channels[i] = write;
        }
    }
}

type BufferPassView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ExtractedCamera,
        &'static DynamicUniformIndex<ShaderToyUniform>,
        Option<&'static ActiveShader>,
    ),
>;

#[derive(SystemParam)]
pub struct BufferPassParams<'w> {
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: Res<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    data_uniforms: Res<'w, ComponentUniforms<ShaderToyUniform>>,
    render_device: Res<'w, RenderDevice>,
    playback: Res<'w, Playback>,
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
    let view = views.into_inner();
    let Some(active) = view.2 else {
        return;
    };
    let mask = active.fragment.buffers;
    if mask == 0 {
        p.channels.reset();
        return;
    }
    let Some(size) = view.0.physical_viewport_size else {
        return;
    };

    let fresh = p
        .targets
        .reset_if_stale(size, (active.fragment.generation, active.epoch()));
    // Channels without a pass read the black dummy.
    let dummy = p.channels.dummy.clone();
    for (i, slot) in p.channels.views.iter_mut().enumerate() {
        if mask & (1 << i) == 0 {
            *slot = dummy.clone();
        }
    }
    // Paused: keep the last results on show (a fresh reset still renders
    // once, so the channels never show freed textures).
    if p.playback.paused && !fresh {
        return;
    }
    p.targets.advance();

    let ids = p.pipelines.ids_for(
        &p.pipeline_cache,
        &p.fullscreen,
        &active.fragment.handle,
        mask,
    );
    let Some(settings_binding) = p.data_uniforms.uniforms().binding() else {
        return;
    };
    let pass = BufferPass {
        device: &p.render_device,
        pipeline_cache: &p.pipeline_cache,
        fullscreen: &p.fullscreen,
        ids,
        mask,
        uniforms: settings_binding,
        offset: view.1.index(),
        dummy: &dummy,
    };
    pass.draw(ctx.command_encoder(), &mut p.targets, &mut p.channels.views);
}
