//! Which shader is on screen, and the playback state that restarts with it.
//!
//! The main world requests a shader through [`ShaderToyFragment`]. Here, each
//! frame, the render world queues that shader's pipelines (the main pass plus
//! one per buffer pass) and keeps drawing the previously active shader until
//! all of them are ready, so a recompile never shows a black frame. A
//! shader whose pipelines fail to build is never activated; the failure is
//! reported back through [`ShaderFeedback`].
//!
//! Activation is also the reset point of shadertoy's per-shader state:
//! `i_frame` restarts at 0, the channel clocks restart and the buffer
//! channels are cleared (see [`super::buffers`]), as on shadertoy after a
//! compile. The Reset button does the same through [`Playback::epoch`].
//! Because the switch happens here, the reset lines up with the first frame
//! the new shader renders, so `i_frame() == 0` initialization works.

use std::sync::{Arc, Mutex};

use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::math::Vec4;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{CachedPipelineState, CachedRenderPipelineId, PipelineCache};
use bevy::render::view::ExtractedView;
use bevy::render::RenderApp;

use super::ShaderToyUniform;
use super::buffers::BufferPipelines;
use super::fullscreen_material::{
    FullscreenMaterialPipeline, FullscreenMaterialPipelineId, ShaderToyFragment,
};
use crate::debugger::MainPassTarget;

/// Playback controls the render world needs, from the main world.
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
#[extract_app(RenderApp)]
pub struct Playback {
    /// While paused, `i_frame` holds still and buffer passes do not run.
    pub paused: bool,
    /// Bumped by the Reset button: restarts `i_frame`, the channel clocks
    /// and the buffers, like activating a new shader does.
    pub epoch: u32,
}

/// The shader drawn for a view (render world), and when it started.
#[derive(Component, Clone, Debug)]
pub struct ActiveShader {
    pub fragment: ShaderToyFragment,
    /// The [`Playback::epoch`] this activation belongs to.
    epoch: u32,
    /// Frames rendered since activation (`i_frame`).
    frame: u32,
    /// `i_time` at activation; channel clocks count from here.
    start_time: f32,
}

impl ActiveShader {
    pub fn epoch(&self) -> u32 {
        self.epoch
    }
}

/// What the render world tells the main world, shared by both.
#[derive(Default)]
pub struct FeedbackState {
    /// The shader currently drawn.
    pub active: Option<ShaderToyFragment>,
    /// A requested shader whose pipelines failed to build: its generation
    /// and the error. Cleared by the main world once reported.
    pub rejected: Option<(u32, String)>,
}

/// Render-to-main feedback, see [`FeedbackState`]. One `Arc` lives in both
/// worlds (inserted by the plugin, not extracted).
#[derive(Resource, Clone, Default)]
pub struct ShaderFeedback(pub Arc<Mutex<FeedbackState>>);

impl ShaderFeedback {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FeedbackState> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Generations that must not be activated: the render error handler marks
/// the shader it blames for a GPU validation error here.
#[derive(Resource, Default)]
pub struct RejectedGeneration(pub Option<u32>);

/// Pipelines a shader needs, and whether they are all built.
struct ShaderPipelines {
    main: CachedRenderPipelineId,
    ready: bool,
    /// The first build error among them, if any.
    error: Option<String>,
}

fn queue_pipelines(
    fragment: &ShaderToyFragment,
    format: bevy::render::render_resource::TextureFormat,
    cache: &PipelineCache,
    fullscreen: &mut FullscreenMaterialPipeline<ShaderToyUniform>,
    buffers: &mut BufferPipelines,
) -> Option<ShaderPipelines> {
    let main = fullscreen
        .main_pipeline(cache, &fragment.handle, format)
        .ok()?;
    let ids = std::iter::once(main).chain(
        buffers
            .ids_for(cache, fullscreen, &fragment.handle, fragment.buffers)
            .into_iter()
            .flatten(),
    );
    let mut ready = true;
    let mut error = None;
    for id in ids {
        match cache.get_render_pipeline_state(id) {
            CachedPipelineState::Ok(_) => {}
            CachedPipelineState::Err(e) => {
                ready = false;
                error.get_or_insert_with(|| e.to_string());
            }
            _ => ready = false,
        }
    }
    Some(ShaderPipelines { main, ready, error })
}

type ActivationViews<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ExtractedView,
        &'static ShaderToyFragment,
        &'static mut ShaderToyUniform,
        Option<&'static ActiveShader>,
    ),
    With<ExtractedCamera>,
>;

/// Pick the shader to draw this frame, patch the per-shader uniforms
/// (`i_frame`, channel clocks and sizes) to match it, and report back.
///
/// Runs in `Queue`, before the uniform buffer is written in
/// `PrepareResources`.
#[allow(clippy::too_many_arguments)] // Bevy system: params are the natural shape
pub fn activate_shader(
    mut commands: Commands,
    mut views: ActivationViews<'_, '_>,
    pipeline_cache: Res<PipelineCache>,
    mut fullscreen: ResMut<FullscreenMaterialPipeline<ShaderToyUniform>>,
    mut buffer_pipelines: ResMut<BufferPipelines>,
    target: MainPassTarget,
    playback: Res<Playback>,
    rejected: Res<RejectedGeneration>,
    feedback: Res<ShaderFeedback>,
) {
    for (entity, view, requested, mut uniform, active) in &mut views {
        let format = target.format(view.target_format);
        let cache = &*pipeline_cache;
        let Some(wanted) =
            queue_pipelines(requested, format, cache, &mut fullscreen, &mut buffer_pipelines)
        else {
            continue;
        };
        let blocked = rejected.0 == Some(requested.generation);
        if let Some(error) = &wanted.error
            && !blocked
        {
            feedback.lock().rejected = Some((requested.generation, error.clone()));
        }

        // Keep drawing the active shader while the request is still being
        // built (or was rejected), as long as the active one still builds.
        let keep = active
            .filter(|a| a.fragment.generation != requested.generation)
            .and_then(|a| {
                queue_pipelines(&a.fragment, format, cache, &mut fullscreen, &mut buffer_pipelines)
                    .filter(|p| p.ready)
                    .map(|p| (a.fragment.clone(), p.main))
            });
        let (fragment, main) = match keep {
            Some(kept) if blocked || !wanted.ready => kept,
            _ if blocked => continue, // nothing drawable; leave the view as is
            _ => (requested.clone(), wanted.main),
        };

        // Restart the per-shader state on activation and on Reset.
        let mut state = match active {
            Some(a) if a.fragment.generation == fragment.generation && a.epoch == playback.epoch => {
                a.clone()
            }
            _ => ActiveShader {
                fragment: fragment.clone(),
                epoch: playback.epoch,
                frame: 0,
                start_time: uniform.time,
            },
        };
        uniform.frame = state.frame;
        let size = Vec4::new(uniform.resolution.x, uniform.resolution.y, 0.0, 0.0);
        let defined = |i: usize| fragment.buffers & (1 << i) != 0;
        let elapsed = uniform.time - state.start_time;
        uniform.channel_time = Vec4::from(std::array::from_fn::<f32, 4, _>(|i| {
            if defined(i) { elapsed } else { 0.0 }
        }));
        uniform.channel_resolution =
            std::array::from_fn(|i| if defined(i) { size } else { Vec4::ZERO });
        if !playback.paused {
            state.frame += 1;
        }

        feedback.lock().active = Some(fragment);
        commands.entity(entity).insert((
            state,
            FullscreenMaterialPipelineId::<ShaderToyUniform>::new(main),
        ));
    }
}
