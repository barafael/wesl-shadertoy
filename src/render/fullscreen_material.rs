//! Vendored and modified from Bevy 0.20's `bevy_core_pipeline::fullscreen_material`
//! (https://github.com/bevyengine/bevy, `crates/bevy_core_pipeline/src/fullscreen_material.rs`),
//! dual-licensed MIT OR Apache-2.0. Attribution: this file is a derivative of
//! that Bevy source file.
//!
//! Modifications for wesl-shadertoy:
//!
//! * The fragment shader is no longer fixed by `FullscreenMaterial::fragment_shader()`
//!   at plugin build time. Instead each extracted view may carry a
//!   [`ShaderToyFragment`] component holding the current `Handle<Shader>`, and
//!   the pipeline is rebuilt whenever that handle changes. This is what makes
//!   runtime shader hot-swapping possible (Bevy's stock material pipelines key
//!   their caches without the shader handle, so they cannot do this).
//! * `FullscreenMaterialPipeline::variants` is built lazily: the first compiled
//!   shader may arrive after `RenderStartup`.
//! * The bind group layout carries four extra `texture_2d` entries (bindings
//!   3..6) for shadertoy's buffer channels, and the bind group is rebuilt
//!   every frame instead of cached, because those channel views ping-pong
//!   per frame (see [`super::buffers`]).

use core::any::type_name;
use core::marker::PhantomData;

use bevy::app::{App, Plugin};
use bevy::asset::Handle;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader, tonemapping::tonemapping};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::error::BevyError;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::{IntoScheduleConfigs, ScheduleConfigs, ScheduleLabel};
use bevy::ecs::system::{BoxedSystem, Commands, Local, Query, Res, ResMut};
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::encase::internal::WriteInto;
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    Canonical, ColorTargetState, ColorWrites, Extent3d, FragmentState, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor,
    Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, Specializer,
    SpecializerKey, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor, Variants, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

/// The currently compiled fragment shader, extracted from the main world onto
/// the camera entity. When this handle changes, the fullscreen pipeline is
/// rebuilt around the new shader.
#[derive(Component, ExtractComponent, Clone, Debug)]
#[extract_app(RenderApp)]
pub struct ShaderToyFragment(pub Handle<Shader>);

#[derive(Default)]
pub struct FullscreenMaterialPlugin<T: FullscreenMaterial> {
    _marker: PhantomData<T>,
}

impl<T: FullscreenMaterial> Plugin for FullscreenMaterialPlugin<T> {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<T>::default(),
            ExtractComponentPlugin::<ShaderToyFragment>::default(),
            UniformComponentPlugin::<T>::default(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .add_systems(RenderStartup, init_pipeline::<T>)
            .add_systems(
                Render,
                prepare_fullscreen_material_pipelines::<T>.in_set(RenderSystems::Prepare),
            );

        let system = T::schedule_configs(fullscreen_material_system::<T>.into_configs());
        render_app.add_systems(T::schedule(), system);
    }
}

/// A trait to define a material that will render to the entire screen using a
/// fullscreen triangle.
///
/// (Modified from Bevy: the fragment shader comes from the per-view
/// [`ShaderToyFragment`] component instead of a static associated function.)
pub trait FullscreenMaterial:
    Component + ExtractComponent<RenderApp> + Clone + Copy + ShaderType + WriteInto + Default
{
    /// The schedule this effect runs in.
    ///
    /// Defaults to [`Core3d`] for 3D post-processing effects.
    fn schedule() -> impl ScheduleLabel + Clone {
        Core3d
    }

    /// Configures this effect's system set and system order.
    ///
    /// By default it's in [`Core3dSystems::PostProcess`] and before
    /// [`tonemapping`].
    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system
            .in_set(Core3dSystems::PostProcess)
            .before(tonemapping)
    }

    /// The fragment entry point to select when the shader module defines
    /// several (wesl-shadertoy addition: shadertoy shaders compile with the
    /// `fragment` entry plus one `fragment_buffer_*` per buffer pass).
    /// `None` lets wgpu pick the module's only entry point.
    fn fragment_entry_point() -> Option<&'static str> {
        None
    }
}

/// The texture views bound at `@group(0) @binding(3..6)` as the shadertoy
/// buffer channels, plus the black 1×1 dummy used for channels the current
/// shader does not define (and as binding-0 filler for buffer passes).
///
/// (wesl-shadertoy addition: [`super::buffers::shadertoy_buffer_pass`]
/// republishes `views` every frame as the buffer passes produce results.)
#[derive(Resource)]
pub struct ChannelViews {
    pub views: [TextureView; 4],
    pub dummy: TextureView,
}

impl ChannelViews {
    /// Point every channel at the black dummy (no buffers defined).
    pub fn reset(&mut self) {
        self.views = std::array::from_fn(|_| self.dummy.clone());
    }
}

#[derive(Resource)]
pub struct FullscreenMaterialPipeline<T: FullscreenMaterial> {
    pub(crate) layout: BindGroupLayoutDescriptor,
    pub(crate) sampler: Sampler,
    pub(crate) vertex: VertexState,
    /// The fragment shader the `variants` cache was built around.
    shader: Option<Handle<Shader>>,
    /// Built lazily once a [`ShaderToyFragment`] has been extracted.
    variants: Option<Variants<RenderPipeline, FullscreenMaterialPipelineSpecializer>>,
    _marker: PhantomData<T>,
}

impl<T: FullscreenMaterial> FullscreenMaterialPipeline<T> {
    fn rebuild_variants(&mut self, shader: Handle<Shader>, label: Option<String>) {
        let desc = RenderPipelineDescriptor {
            label: label.map(std::borrow::Cow::Owned),
            layout: vec![self.layout.clone()],
            vertex: self.vertex.clone(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: T::fragment_entry_point().map(std::borrow::Cow::from),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            ..Default::default()
        };
        self.shader = Some(shader);
        self.variants = Some(Variants::new(FullscreenMaterialPipelineSpecializer, desc));
    }
}

struct FullscreenMaterialPipelineSpecializer;

#[derive(PartialEq, Eq, Hash, Clone, Copy, SpecializerKey)]
struct FullscreenMaterialPipelineKey {
    target_format: TextureFormat,
}

impl Specializer<RenderPipeline> for FullscreenMaterialPipelineSpecializer {
    type Key = FullscreenMaterialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        let fragment = descriptor.fragment_mut()?;
        let color_target_state = ColorTargetState {
            format: key.target_format,
            blend: None,
            write_mask: ColorWrites::ALL,
        };
        fragment.set_target(0, color_target_state);
        Ok(key)
    }
}

fn init_pipeline<T: FullscreenMaterial>(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "fullscreen_material_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                // `i_sampler`. Non-filtering, so shaders may pair it with the
                // unfilterable channels below: a filtering binding passes naga
                // but makes wgpu reject the pipeline. The sampler itself is
                // `SamplerDescriptor::default()`, i.e. nearest.
                sampler(SamplerBindingType::NonFiltering),
                uniform_buffer::<T>(true),
                // shadertoy buffer channels A–D (i_channel_0()..i_channel_3()).
                // `Rgba32Float` views are unfilterable without the
                // `float32-filterable` feature, so the layout must not claim
                // otherwise; `textureLoad` works regardless.
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor::default());

    // Black 1×1 stand-in for channels the shader does not define. Textures
    // read as zero before anything is written, so a plain TEXTURE_BINDING
    // usage is enough; Rgba32Float keeps it format-compatible with buffers.
    let dummy = render_device.create_texture(&TextureDescriptor {
        label: Some("shadertoy_channel_dummy"),
        size: Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba32Float,
        usage: TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let dummy_view = dummy.create_view(&TextureViewDescriptor::default());
    commands.insert_resource(ChannelViews {
        views: std::array::from_fn(|_| dummy_view.clone()),
        dummy: dummy_view,
    });

    commands.insert_resource(FullscreenMaterialPipeline::<T> {
        layout,
        sampler,
        vertex: fullscreen_shader.to_vertex_state(),
        shader: None,
        variants: None,
        _marker: PhantomData,
    });
}

#[derive(Component)]
pub struct FullscreenMaterialPipelineId<T: FullscreenMaterial>(
    pub CachedRenderPipelineId,
    PhantomData<T>,
);

/// View query data for [`prepare_fullscreen_material_pipelines`], extracted
/// into an alias to keep the system signature readable (upstream Bevy has the
/// same tuple inline; this is a deviation only to silence `type_complexity`).
type PipelineViews<'w, 's, T> = Query<
    'w,
    's,
    (
        Entity,
        &'static ExtractedView,
        Option<&'static T>,
        Option<&'static ShaderToyFragment>,
    ),
    With<ExtractedCamera>,
>;

fn prepare_fullscreen_material_pipelines<'w, 's, T: FullscreenMaterial>(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipeline: ResMut<FullscreenMaterialPipeline<T>>,
    views: PipelineViews<'w, 's, T>,
) -> Result<(), BevyError> {
    for (entity, view, material, fragment) in &views {
        let (Some(_material), Some(fragment)) = (material, fragment) else {
            commands
                .entity(entity)
                .remove::<FullscreenMaterialPipelineId<T>>();
            continue;
        };

        // Rebuild the variant cache whenever the current shader handle
        // changes. A fresh `Variants` has an empty cache, so `specialize`
        // below queues a brand-new pipeline for the new shader.
        let stale = match &pipeline.shader {
            Some(current) => current.id() != fragment.0.id(),
            None => true,
        };
        if stale || pipeline.variants.is_none() {
            let label = format!("fullscreen_material_pipeline<{}>", type_name::<T>());
            pipeline.rebuild_variants(fragment.0.clone(), Some(label));
        }

        let Some(variants) = pipeline.variants.as_mut() else {
            continue;
        };

        let pipeline_key = FullscreenMaterialPipelineKey {
            target_format: view.target_format,
        };
        let pipeline_id = variants.specialize(&pipeline_cache, pipeline_key)?;

        commands
            .entity(entity)
            .insert(FullscreenMaterialPipelineId::<T>(pipeline_id, PhantomData));
    }

    Ok(())
}

/// View query data for [`fullscreen_material_system`] (see [`PipelineViews`]
/// for why these queries are aliased).
type MaterialView<'w, 's, T> = ViewQuery<
    'w,
    's,
    (
        &'static ViewTarget,
        &'static DynamicUniformIndex<T>,
        &'static FullscreenMaterialPipelineId<T>,
    ),
>;

/// (Modified from Bevy: instead of a cached [`FullscreenMaterialBindGroup`],
/// the bind group is created every frame so it always carries the current
/// [`ChannelViews`] — the buffer channels ping-pong each frame.)
#[allow(clippy::too_many_arguments)] // Bevy system: params are the natural shape
pub fn fullscreen_material_system<'w, 's, T: FullscreenMaterial>(
    view: MaterialView<'w, 's, T>,
    pipeline_cache: Res<PipelineCache>,
    fullscreen_pipeline: Res<FullscreenMaterialPipeline<T>>,
    render_device: Res<RenderDevice>,
    data_uniforms: Res<ComponentUniforms<T>>,
    channels: Res<ChannelViews>,
    mut ctx: RenderContext,
    mut pass_label: Local<String>,
) {
    let (view_target, settings_index, pipeline_id) = view.into_inner();

    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
        return;
    };
    let Some(settings_binding) = data_uniforms.uniforms().binding() else {
        return;
    };

    let post_process = view_target.post_process_write();
    let source = post_process.source;
    let destination = post_process.destination;

    let bind_group = render_device.create_bind_group(
        "fullscreen_material_bind_group",
        &pipeline_cache.get_bind_group_layout(&fullscreen_pipeline.layout),
        &BindGroupEntries::sequential((
            source,
            &fullscreen_pipeline.sampler,
            settings_binding,
            &channels.views[0],
            &channels.views[1],
            &channels.views[2],
            &channels.views[3],
        )),
    );

    if pass_label.is_empty() {
        *pass_label = format!("fullscreen_material_pass<{}>", type_name::<T>());
    }
    let pass_descriptor = RenderPassDescriptor {
        label: Some(&pass_label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    };

    {
        let diagnostics = ctx.diagnostic_recorder();
        let diagnostics = diagnostics.as_deref();

        let mut render_pass = ctx.command_encoder().begin_render_pass(&pass_descriptor);
        let pass_span = diagnostics.pass_span(&mut render_pass, pass_label.clone());

        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, &bind_group, &[settings_index.index()]);
        render_pass.draw(0..3, 0..1);
        pass_span.end(&mut render_pass);
    }
}
