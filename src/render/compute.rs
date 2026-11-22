//! Compute passes (`compute_a`..`compute_d`) and the storage buffers they
//! share with the fragment passes.
//!
//! When the editor shader defines `compute_*` functions, the compiler emits
//! an `entry_compute_*` entry point for each (see [`crate::compile`]), and
//! this module dispatches them, A→D, before the buffer passes
//! ([`super::buffers`]) and the image pass. Every pass sees the shader's
//! storage buffers (`@group(1) @binding(0..3)`), which live as long as its
//! activation: they start zeroed when the shader is activated, on Reset and,
//! for the runtime-sized ones (one element per pixel), on resize. While
//! paused, the passes hold still, like the buffer passes.
//!
//! Compute passes read the channels as the previous frame left them, since
//! the buffer passes run after them.
//!
//! Only GPUs with compute support get any of this ([`GpuCompute`]): WebGPU
//! and the native backends, not WebGL2. Elsewhere the pipeline layout has
//! no storage group, and the main world refuses shaders that need one.

use bevy::asset::{AssetId, Handle};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut, SystemParam};
use bevy::log::warn;
use bevy::math::UVec2;
use bevy::platform::collections::HashMap;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayout, Buffer, BufferDescriptor, BufferUsages,
    CachedComputePipelineId, CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor,
    DownlevelFlags, PipelineCache, TextureView,
};
use bevy::render::renderer::{RenderAdapter, RenderContext, RenderDevice, ViewQuery};
use bevy::shader::Shader;

use super::ShaderToyUniform;
use super::active::{ActiveShader, Playback};
use super::fullscreen_material::{ChannelViews, FullscreenMaterialPipeline, StorageLayout};
use crate::compile::{self, Compute, STORAGE_SLOTS, StorageSize};

/// Whether the GPU runs compute passes. In both worlds, set once the
/// renderer exists ([`super::ShaderToyRenderPlugin`]'s `finish`).
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct GpuCompute(pub bool);

impl GpuCompute {
    /// The compile error for a shader with compute passes or storage
    /// buffers, where they cannot run.
    pub fn refuse(&self, compute: &Compute) -> Option<compile::CompileError> {
        (!self.0 && compute.is_used()).then(|| {
            compile::CompileError::Compute(
                "Compute passes and storage buffers need WebGPU (or a native build); \
                 this GPU runs WebGL2."
                    .into(),
            )
        })
    }
}

/// Compute shaders, storage buffers the fragment stage may write, and the
/// four storage bindings per stage the storage group takes.
pub fn supported(adapter: &RenderAdapter, device: &RenderDevice) -> bool {
    let flags = adapter.get_downlevel_capabilities().flags;
    flags.contains(DownlevelFlags::COMPUTE_SHADERS | DownlevelFlags::FRAGMENT_WRITABLE_STORAGE)
        && device.limits().max_storage_buffers_per_shader_stage >= STORAGE_SLOTS as u32
}

/// What a set of storage buffers was made for: the sizes, the viewport
/// (only if a buffer is sized by it) and the `(generation, epoch)` the
/// contents belong to.
type StorageKey = ([Option<StorageSize>; STORAGE_SLOTS], UVec2, (u32, u32));

/// One activation's storage buffers.
#[derive(Default)]
pub(crate) struct StorageBuffers {
    key: Option<StorageKey>,
    buffers: [Option<Buffer>; STORAGE_SLOTS],
}

impl StorageBuffers {
    /// Make the buffers match `storage` at viewport `size` for `owner`.
    /// Returns whether they were (re)created, which zeroes them.
    pub fn prepare(
        &mut self,
        device: &RenderDevice,
        storage: &[Option<StorageSize>; STORAGE_SLOTS],
        size: UVec2,
        owner: (u32, u32),
    ) -> bool {
        let per_pixel = storage
            .iter()
            .any(|s| matches!(s, Some(StorageSize::PerPixel { .. })));
        let size = if per_pixel {
            size.max(UVec2::ONE)
        } else {
            UVec2::ZERO
        };
        let key = (*storage, size, owner);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        let max = device.limits().max_storage_buffer_binding_size;
        self.buffers = std::array::from_fn(|i| {
            let bytes = storage[i]?.bytes(size.x, size.y);
            if bytes > max {
                // Only per-pixel buffers get here (fixed ones are capped at
                // compile time below every WebGPU device's limit): accesses
                // past the end are out of bounds, which WGSL keeps memory-safe
                // but leaves implementation-defined (clamped or dropped).
                warn!(
                    "storage buffer at @binding({i}) needs {} MiB at {}×{}, more than \
                     this GPU's {} MiB; it is cut short",
                    bytes >> 20,
                    size.x,
                    size.y,
                    max >> 20
                );
            }
            Some(device.create_buffer(&BufferDescriptor {
                label: Some(&format!("shadertoy_storage_{i}")),
                size: (bytes.min(max) & !3).max(16),
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            }))
        });
        true
    }

    /// The storage group, with `dummy` in the slots the shader leaves empty.
    pub fn bind_group(
        &self,
        device: &RenderDevice,
        layout: &BindGroupLayout,
        dummy: &Buffer,
    ) -> BindGroup {
        let slot = |i: usize| {
            self.buffers[i]
                .as_ref()
                .unwrap_or(dummy)
                .as_entire_binding()
        };
        device.create_bind_group(
            "shadertoy_storage_bind_group",
            layout,
            &BindGroupEntries::sequential((slot(0), slot(1), slot(2), slot(3))),
        )
    }
}

/// The main view's storage buffers, and this frame's storage group for
/// every pass of the shader on screen ([`shadertoy_compute_pass`] sets it
/// before the buffer and image passes draw).
#[derive(Default, Resource)]
pub struct MainStorage {
    buffers: StorageBuffers,
    pub(crate) bind_group: Option<BindGroup>,
}

/// The `entry_compute_*` pipelines, one per shader handle and pass, like
/// [`super::buffers::BufferPipelines`].
#[derive(Default, Resource)]
pub(crate) struct ComputePipelines {
    ids: HashMap<(AssetId<Shader>, usize), CachedComputePipelineId>,
}

impl ComputePipelines {
    /// The pipelines for the passes in `mask`, queued on first request;
    /// none where the GPU lacks compute.
    pub fn ids_for(
        &mut self,
        pipeline_cache: &PipelineCache,
        fullscreen: &FullscreenMaterialPipeline<ShaderToyUniform>,
        shader: &Handle<Shader>,
        mask: u8,
    ) -> [Option<CachedComputePipelineId>; 4] {
        std::array::from_fn(|i| {
            if mask & (1 << i) == 0 || fullscreen.storage.is_none() {
                return None;
            }
            let id = *self.ids.entry((shader.id(), i)).or_insert_with(|| {
                let name = compile::COMPUTE_NAMES[i];
                pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some(format!("shadertoy_compute_pipeline_{name}").into()),
                    layout: fullscreen.layouts(),
                    shader: shader.clone(),
                    entry_point: Some(compile::compute_entry(name).into()),
                    zero_initialize_workgroup_memory: true,
                    ..Default::default()
                })
            });
            Some(id)
        })
    }
}

/// One frame's compute passes for one shader.
pub(crate) struct ComputePass<'a> {
    pub pipeline_cache: &'a PipelineCache,
    /// From [`ComputePipelines::ids_for`].
    pub ids: [Option<CachedComputePipelineId>; 4],
    pub compute: &'a Compute,
    /// The viewport, for per-pixel passes.
    pub size: UVec2,
    /// Group 0 and its uniform offset.
    pub group0: &'a BindGroup,
    pub offset: u32,
    pub storage: &'a BindGroup,
}

impl ComputePass<'_> {
    /// Dispatch the defined passes, A→D. A pass whose pipeline is still
    /// compiling is skipped this frame.
    pub fn dispatch(&self, encoder: &mut CommandEncoder) {
        for (i, (pass, id)) in self.compute.passes.iter().zip(self.ids).enumerate() {
            let (Some(pass), Some(id)) = (pass, id) else {
                continue;
            };
            let Some(pipeline) = self.pipeline_cache.get_compute_pipeline(id) else {
                continue;
            };
            let [x, y, z] = pass.workgroups(self.size.x, self.size.y);
            if x * y * z == 0 {
                continue;
            }
            let mut cpass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some(&format!(
                    "shadertoy_compute_pass_{}",
                    compile::COMPUTE_NAMES[i]
                )),
                timestamp_writes: None,
            });
            cpass.set_pipeline(pipeline);
            cpass.set_bind_group(0, self.group0, &[self.offset]);
            cpass.set_bind_group(1, self.storage, &[]);
            cpass.dispatch_workgroups(x, y, z);
        }
    }
}

/// Group 0 for a compute pass: no source, `channels` as they stand.
pub(crate) fn compute_group0(
    device: &RenderDevice,
    cache: &PipelineCache,
    fullscreen: &FullscreenMaterialPipeline<ShaderToyUniform>,
    uniforms: bevy::render::render_resource::BindingResource,
    channels: [&TextureView; 4],
    dummy: &TextureView,
) -> BindGroup {
    fullscreen.bind_group(
        device,
        cache,
        "shadertoy_compute_bind_group",
        dummy,
        uniforms,
        channels,
        None,
    )
}

type ComputePassView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ExtractedCamera,
        &'static DynamicUniformIndex<ShaderToyUniform>,
        Option<&'static ActiveShader>,
    ),
>;

#[derive(SystemParam)]
pub struct ComputePassParams<'w> {
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: Res<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    data_uniforms: Res<'w, ComponentUniforms<ShaderToyUniform>>,
    render_device: Res<'w, RenderDevice>,
    playback: Res<'w, Playback>,
    channels: Res<'w, ChannelViews>,
    storage: ResMut<'w, MainStorage>,
    pipelines: ResMut<'w, ComputePipelines>,
}

/// Prepare the shader on screen's storage buffers, publish its storage
/// group in [`MainStorage`] and dispatch its compute passes (A→D).
///
/// Runs in `Core3d`'s post-process set, before
/// [`super::buffers::shadertoy_buffer_pass`], recording into the same
/// command encoder so the passes land in order.
pub fn shadertoy_compute_pass(
    views: ComputePassView<'_, '_>,
    mut p: ComputePassParams<'_>,
    mut ctx: RenderContext,
) {
    p.storage.bind_group = None;
    let Some(StorageLayout { layout, dummy }) = &p.fullscreen.storage else {
        return;
    };
    let (camera, index, active) = views.into_inner();
    let (Some(active), Some(size)) = (active, camera.physical_viewport_size) else {
        return;
    };
    let compute = &active.fragment.compute;
    let device = &*p.render_device;
    let cache = &*p.pipeline_cache;
    let fresh = p.storage.buffers.prepare(
        device,
        &compute.storage,
        size,
        (active.fragment.generation, active.epoch()),
    );
    let storage = p
        .storage
        .buffers
        .bind_group(device, &cache.get_bind_group_layout(layout), dummy);
    p.storage.bind_group = Some(storage.clone());

    // Paused: hold still (a fresh start still runs once, to initialise).
    if compute.mask() == 0 || (p.playback.paused && !fresh) {
        return;
    }
    let ids = p.pipelines.ids_for(
        cache,
        &p.fullscreen,
        &active.fragment.handle,
        compute.mask(),
    );
    let Some(uniforms) = p.data_uniforms.uniforms().binding() else {
        return;
    };
    // The channels from the previous frame; from an earlier shader, black.
    let black = &p.channels.dummy;
    let channels = if fresh {
        [black; 4]
    } else {
        p.channels.views.each_ref()
    };
    let group0 = compute_group0(device, cache, &p.fullscreen, uniforms, channels, black);
    ComputePass {
        pipeline_cache: cache,
        ids,
        compute,
        size,
        group0: &group0,
        offset: index.index(),
        storage: &storage,
    }
    .dispatch(ctx.command_encoder());
}
