//! Live thumbnails: the example browser's cards, Evolve mode's variants and
//! the Graph window's per-`let` thumbnails ([`crate::graph`]).
//!
//! The main world ([`crate::browser`]) compiles each example on first sight
//! into a shader asset of its own and, every frame, lists the cards to draw
//! as [`PreviewJobs`]: the hovered card plus a few of the visible ones in
//! turn. [`crate::evolve`] adds its nine variants of the editor's shader.
//! Here each job runs its shader's buffer passes (A→D, with their own
//! ping-pong targets per card) and its image pass into the card's image,
//! which egui shows. The pipelines come from the same caches as the main
//! shader's, keyed by the shader handle.

use std::sync::{Arc, Mutex};

use bevy::asset::{AssetId, Handle};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut, SystemParam};
use bevy::image::Image;
use bevy::math::UVec2;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::render::RenderApp;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    BindGroupEntries, CachedRenderPipelineId, CommandEncoderDescriptor, DynamicUniformBuffer,
    Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, TextureFormat,
    TextureView,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::shader::Shader;

use super::ShaderToyUniform;
use super::buffers::{BufferPass, BufferPipelines, BufferTargets};
use super::fullscreen_material::{
    ChannelViews, FullscreenMaterialPipeline, FullscreenMaterialPipelineId,
};

/// Size of every thumbnail, in pixels.
pub const PREVIEW_SIZE: UVec2 = UVec2::new(320, 180);

/// Thumbnail format: what the view target and egui images use.
pub const PREVIEW_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// Who a thumbnail belongs to; each keeps its own frame counter and
/// buffer targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewKey {
    /// A browser card: an index into [`crate::examples::ALL`].
    Example(usize),
    /// An Evolve grid cell, row by row.
    Evolve(usize),
    /// A Graph window thumbnail slot ([`crate::graph`]).
    Graph(usize),
}

/// One thumbnail to draw this frame.
#[derive(Clone)]
pub struct PreviewJob {
    pub card: PreviewKey,
    pub image: AssetId<Image>,
    /// The image's size in pixels ([`PREVIEW_SIZE`] for browser cards).
    pub size: UVec2,
    /// Changing it restarts the card's `i_frame` and buffers (the browser's
    /// shaders never change; Evolve's follows the editor's compiles).
    pub generation: u32,
    pub shader: Handle<Shader>,
    /// Bit per buffer pass the example defines.
    pub buffers: u8,
    /// Its uniforms; `frame` is filled in here, per card.
    pub uniform: ShaderToyUniform,
}

/// The thumbnails to draw this frame, from the main world.
#[derive(Resource, ExtractResource, Clone, Default)]
#[extract_app(RenderApp)]
pub struct PreviewJobs(pub Vec<PreviewJob>);

/// Cards whose thumbnail has been drawn at least once, so the browser can
/// stop showing them as loading. One `Arc` lives in both worlds.
#[derive(Resource, Clone, Default)]
pub struct PreviewFeedback(pub Arc<Mutex<HashSet<PreviewKey>>>);

impl PreviewFeedback {
    pub fn drawn(&self, card: PreviewKey) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&card)
    }

    fn mark(&self, card: PreviewKey) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(card);
    }
}

/// Render-world state per card: its frame counter and buffer targets.
#[derive(Resource, Default)]
pub struct PreviewState {
    cards: HashMap<PreviewKey, Card>,
    uniforms: DynamicUniformBuffer<ShaderToyUniform>,
    /// Cards the GPU rejected, with the generation it rejected: not drawn
    /// again until their shader changes.
    failed: HashMap<PreviewKey, u32>,
    /// Calls of [`preview_pass`] so far, i.e. frames.
    frame: u64,
}

/// Frames a card may go without a job before it is forgotten, freeing its
/// buffer targets (its `i_frame` restarts if it comes back): far longer
/// than the round-robin takes to return to a visible card, so only cards
/// scrolled away or whose window closed go.
const IDLE_FRAMES: u64 = 300;

impl PreviewState {
    /// Stop drawing `card` at `generation` (see [`card_from_error`]).
    pub fn fail(&mut self, card: PreviewKey, generation: u32) {
        self.failed.insert(card, generation);
    }
}

/// Each card records into a command encoder of its own, labelled with this
/// prefix, the card and its generation. wgpu quotes the label in its
/// errors, so a GPU error in a thumbnail is pinned on that card rather
/// than on the editor's shader.
const ENCODER_PREFIX: &str = "shadertoy_preview:";

fn encoder_label(card: PreviewKey, generation: u32) -> String {
    let (kind, i) = match card {
        PreviewKey::Example(i) => ("example", i),
        PreviewKey::Evolve(i) => ("evolve", i),
        PreviewKey::Graph(i) => ("graph", i),
    };
    format!("{ENCODER_PREFIX}{kind}:{i}:{generation}")
}

/// The card and generation whose encoder a wgpu error description names.
pub fn card_from_error(description: &str) -> Option<(PreviewKey, u32)> {
    let start = description.find(ENCODER_PREFIX)? + ENCODER_PREFIX.len();
    let mut parts = description[start..].splitn(3, ':');
    let (kind, i) = (parts.next()?, parts.next()?.parse().ok()?);
    let generation = parts.next()?;
    let digits = generation.find(|c: char| !c.is_ascii_digit());
    let generation = generation[..digits.unwrap_or(generation.len())]
        .parse()
        .ok()?;
    let card = match kind {
        "example" => PreviewKey::Example(i),
        "evolve" => PreviewKey::Evolve(i),
        "graph" => PreviewKey::Graph(i),
        _ => return None,
    };
    Some((card, generation))
}

#[derive(Default)]
struct Card {
    /// [`PreviewState::frame`] when it last had a job.
    seen: u64,
    /// `i_frame`: thumbnails drawn so far.
    frame: u32,
    /// The job's [`PreviewJob::generation`] that `frame` counts for.
    generation: u32,
    targets: BufferTargets,
    channels: Option<[TextureView; 4]>,
}

#[derive(SystemParam)]
pub struct PreviewParams<'w> {
    jobs: Res<'w, PreviewJobs>,
    state: ResMut<'w, PreviewState>,
    feedback: Res<'w, PreviewFeedback>,
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: ResMut<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    buffer_pipelines: ResMut<'w, BufferPipelines>,
    channels: Res<'w, ChannelViews>,
    images: Res<'w, RenderAssets<GpuImage>>,
    device: Res<'w, RenderDevice>,
    queue: Res<'w, RenderQueue>,
}

/// A job whose pipelines are all built, with its uniform offset.
struct Ready<'a> {
    job: &'a PreviewJob,
    main: CachedRenderPipelineId,
    buffers: [Option<CachedRenderPipelineId>; 4],
    offset: u32,
}

/// Draw this frame's [`PreviewJobs`]. Runs in `Core3d` post-processing for
/// the shadertoy view (so once per frame), before the main pass.
pub fn preview_pass(
    _view: ViewQuery<&FullscreenMaterialPipelineId<ShaderToyUniform>>,
    mut p: PreviewParams,
    mut ctx: RenderContext,
) {
    let state = &mut *p.state;
    state.frame += 1;
    let now = state.frame;
    state.cards.retain(|_, card| now - card.seen < IDLE_FRAMES);
    if p.jobs.0.is_empty() {
        return;
    }
    let cache = &*p.pipeline_cache;

    // Queue every job's pipelines; draw only those already built.
    state.uniforms.clear();
    let mut ready = Vec::new();
    for job in &p.jobs.0 {
        if state.failed.get(&job.card) == Some(&job.generation) {
            continue;
        }
        let Ok(main) = p
            .fullscreen
            .main_pipeline(cache, &job.shader, PREVIEW_FORMAT)
        else {
            continue;
        };
        let buffers = p
            .buffer_pipelines
            .ids_for(cache, &p.fullscreen, &job.shader, job.buffers);
        let built = std::iter::once(Some(main))
            .chain(buffers)
            .flatten()
            .all(|id| cache.get_render_pipeline(id).is_some());
        if !built || p.images.get(job.image).is_none() {
            continue;
        }
        let card = state.cards.entry(job.card).or_default();
        card.seen = now;
        if card.generation != job.generation {
            card.generation = job.generation;
            card.frame = 0;
        }
        let mut uniform = job.uniform;
        uniform.frame = card.frame;
        let offset = state.uniforms.push(&uniform);
        ready.push(Ready {
            job,
            main,
            buffers,
            offset,
        });
    }
    if ready.is_empty() {
        return;
    }
    state.uniforms.write_buffer(&p.device, &p.queue);
    let Some(uniforms) = state.uniforms.binding() else {
        return;
    };
    let layout = cache.get_bind_group_layout(&p.fullscreen.layout);
    let dummy = &p.channels.dummy;

    for r in ready {
        let (Some(image), Some(main)) =
            (p.images.get(r.job.image), cache.get_render_pipeline(r.main))
        else {
            continue;
        };
        let card = state.cards.get_mut(&r.job.card).expect("added above");
        let channels = card
            .channels
            .get_or_insert_with(|| std::array::from_fn(|_| dummy.clone()));
        let label = encoder_label(r.job.card, r.job.generation);
        let mut encoder = p.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some(&label),
        });
        if r.job.buffers != 0 {
            // Fresh targets on the first frame, on resize and with a new
            // generation.
            card.targets
                .reset_if_stale(r.job.size, (r.job.generation, 0));
            card.targets.advance();
            BufferPass {
                device: &p.device,
                pipeline_cache: cache,
                fullscreen: &p.fullscreen,
                ids: r.buffers,
                mask: r.job.buffers,
                uniforms: uniforms.clone(),
                offset: r.offset,
                dummy,
            }
            .draw(&mut encoder, &mut card.targets, channels);
        }

        let bind_group = p.device.create_bind_group(
            "preview_bind_group",
            &layout,
            &BindGroupEntries::sequential((
                dummy,
                &p.fullscreen.sampler,
                uniforms.clone(),
                &channels[0],
                &channels[1],
                &channels[2],
                &channels[3],
                &p.fullscreen.ui_dummy,
                &p.fullscreen.ui_sampler,
            )),
        );
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("preview_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &image.texture_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(main);
        pass.set_bind_group(0, &bind_group, &[r.offset]);
        pass.draw(0..3, 0..1);
        drop(pass);
        ctx.add_command_buffer(encoder.finish());

        card.frame += 1;
        p.feedback.mark(r.job.card);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card comes back out of a wgpu error quoting its encoder label.
    #[test]
    fn card_survives_the_error_text() {
        for card in [
            PreviewKey::Example(12),
            PreviewKey::Evolve(4),
            PreviewKey::Graph(0),
        ] {
            let description = format!(
                "Validation Error\n\nCaused by:\n  In a CommandEncoder, label = '{}'\n    \
                 In a set_pipeline command",
                encoder_label(card, 37)
            );
            assert_eq!(card_from_error(&description), Some((card, 37)));
        }
        let main = "In a CommandEncoder, label = 'wesl_shadertoy::render::ui_pass::ui_melt_pass'";
        assert_eq!(card_from_error(main), None);
    }
}
