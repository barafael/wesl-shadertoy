//! Frosted-glass backdrop for the UI: a blurred copy of the finished frame
//! that egui paints under translucent windows and panels.
//!
//! After the main pass (and the debugger overlay), three fullscreen passes
//! fill the [`Blur::image`] asset, a quarter of the viewport in each
//! direction: a 4×4 box downsample of the view, then a separable Gaussian,
//! horizontal into a scratch texture and vertical back into the image. egui
//! samples it as a user texture (bilinear, which smooths the upscale), so the
//! blur costs a few small passes per frame. The settings slider sets its
//! [`Blur::sigma`]; at 0 nothing runs and the image shrinks to 1×1.

use bevy::asset::{Assets, Handle};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, Res, ResMut, Single, SystemParam};
use bevy::image::Image;
use bevy::math::{UVec2, Vec4};
use bevy::render::RenderApp;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor, UniformBuffer,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::shader::Shader;
use bevy::window::{PrimaryWindow, Window};

use crate::render::ShaderToyUniform;
use crate::render::fullscreen_material::{
    FullscreenMaterialPipeline, FullscreenMaterialPipelineId,
};

/// Format of the blurred image and the scratch texture: renderable and
/// filterable everywhere (WebGL2 included), and what egui expects of an
/// image texture.
const FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// The viewport is blurred at 1/DOWNSCALE resolution in each direction.
const DOWNSCALE: u32 = 4;

const BLUR_WGSL: &str = r#"
struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
// x: the Gaussian's sigma in texels of the downscaled image.
@group(0) @binding(2) var<uniform> params: vec4<f32>;

// Four bilinear taps one source texel off the centre: the mean of the 4x4
// source texels under each output texel of the 4x downscale.
@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = 1. / vec2<f32>(textureDimensions(src));
    var sum = vec3<f32>(0.);
    for (var i = 0; i < 4; i = i + 1) {
        let offset = vec2<f32>(f32(i & 1), f32(i >> 1u)) * 2. - 1.;
        sum = sum + textureSampleLevel(src, src_sampler, in.uv + offset * texel, 0.).rgb;
    }
    return vec4<f32>(0.25 * sum, 1.);
}

// Gaussian cut at 2.5 sigma; MAX_RADIUS covers MAX_SIGMA (in texels).
const MAX_RADIUS: i32 = 30;

fn gaussian(uv: vec2<f32>, direction: vec2<f32>) -> vec4<f32> {
    let sigma = params.x;
    let radius = min(i32(ceil(2.5 * sigma)), MAX_RADIUS);
    let step = direction / vec2<f32>(textureDimensions(src));
    var sum = vec3<f32>(0.);
    var weights = 0.;
    for (var i = -MAX_RADIUS; i <= MAX_RADIUS; i = i + 1) {
        if (abs(i) > radius) {
            continue;
        }
        let x = f32(i);
        let w = exp(-x * x / (2. * sigma * sigma));
        sum = sum + w * textureSampleLevel(src, src_sampler, uv + x * step, 0.).rgb;
        weights = weights + w;
    }
    return vec4<f32>(sum / weights, 1.);
}

@fragment
fn blur_h(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return gaussian(in.uv, vec2<f32>(1., 0.));
}

@fragment
fn blur_v(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return gaussian(in.uv, vec2<f32>(0., 1.));
}
"#;

/// The blurred backdrop: how strong it is (the settings slider), the image
/// egui paints, and the shader that fills it. Created in the main world and
/// extracted to the render world for [`blur_pass`].
#[derive(Resource, ExtractResource, Clone, Default)]
#[extract_app(RenderApp)]
pub struct Blur {
    /// The Gaussian's sigma in viewport (physical) pixels; 0 is off.
    pub sigma: f32,
    pub image: Handle<Image>,
    shader: Handle<Shader>,
}

/// Main-world startup: the (1×1 until sized) image and the shader asset.
pub fn setup_blur(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut shaders: ResMut<Assets<Shader>>,
) {
    commands.insert_resource(Blur {
        sigma: 0.0,
        image: images.add(Image::new_target_texture(1, 1, FORMAT, None)),
        shader: shaders.add(Shader::from_wgsl(BLUR_WGSL, "shadertoy://blur")),
    });
}

/// Keep the image at a quarter of the window while the blur is on, 1×1
/// while it is off.
pub fn size_blur_image(
    window: Single<&Window, bevy::ecs::query::With<PrimaryWindow>>,
    blur: Res<Blur>,
    mut images: ResMut<Assets<Image>>,
) {
    let size = if blur.sigma > 0.0 {
        UVec2::new(window.physical_width(), window.physical_height()) / DOWNSCALE
    } else {
        UVec2::ONE
    }
    .max(UVec2::ONE);
    if images.get(&blur.image).is_some_and(|i| i.size() != size) {
        images
            .insert(
                &blur.image,
                Image::new_target_texture(size.x, size.y, FORMAT, None),
            )
            .expect("the blur image handle stays valid");
    }
}

/// Render-world resources for [`blur_pass`].
#[derive(Resource)]
pub struct BlurPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    /// The shader's `params`.
    params: UniformBuffer<Vec4>,
    /// Downsample, horizontal and vertical blur, queued on first use.
    pipelines: Option<[CachedRenderPipelineId; 3]>,
    /// Holds the horizontal pass, sized like the image.
    scratch: Option<(UVec2, Texture, TextureView)>,
}

pub fn setup_blur_pipelines(mut commands: Commands, device: Res<RenderDevice>) {
    let layout = BindGroupLayoutDescriptor::new(
        "blur_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<Vec4>(false),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("blur_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..Default::default()
    });
    commands.insert_resource(BlurPipelines {
        layout,
        sampler,
        params: UniformBuffer::default(),
        pipelines: None,
        scratch: None,
    });
}

type BlurView<'w, 's> = ViewQuery<
    'w,
    's,
    (
        &'static ViewTarget,
        // Only views the fullscreen-material plugin drives are blurred.
        &'static FullscreenMaterialPipelineId<ShaderToyUniform>,
    ),
>;

/// Resources [`blur_pass`] reads, bundled into one parameter.
#[derive(SystemParam)]
pub struct BlurParams<'w> {
    blur: Res<'w, Blur>,
    res: ResMut<'w, BlurPipelines>,
    pipeline_cache: Res<'w, PipelineCache>,
    fullscreen: Res<'w, FullscreenMaterialPipeline<ShaderToyUniform>>,
    images: Res<'w, RenderAssets<GpuImage>>,
    device: Res<'w, RenderDevice>,
    queue: Res<'w, RenderQueue>,
}

/// Largest sigma the shader covers, in downscaled texels (see `MAX_RADIUS`
/// in the shader: 2.5 sigma).
const MAX_SIGMA: f32 = 12.0;

/// Blur the finished frame into the [`Blur::image`]. Runs after the main
/// pass and the debugger overlay and before egui, recording into the same
/// command encoder so the passes land in that order.
pub fn blur_pass(view: BlurView<'_, '_>, mut p: BlurParams, mut ctx: RenderContext) {
    if p.blur.sigma <= 0.0 {
        return;
    }
    let (view_target, _marker) = view.into_inner();
    let Some(image) = p.images.get(&p.blur.image) else {
        return;
    };
    let size = image.size_2d();
    if size == UVec2::ONE {
        return; // not sized for the viewport yet
    }

    let res = &mut *p.res;
    let pipelines = *res.pipelines.get_or_insert_with(|| {
        ["downsample", "blur_h", "blur_v"].map(|entry| {
            p.pipeline_cache
                .queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(format!("blur_{entry}_pipeline").into()),
                    layout: vec![res.layout.clone()],
                    vertex: p.fullscreen.vertex.clone(),
                    fragment: Some(FragmentState {
                        shader: p.blur.shader.clone(),
                        entry_point: Some(entry.into()),
                        targets: vec![Some(ColorTargetState {
                            format: FORMAT,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        ..Default::default()
                    }),
                    ..Default::default()
                })
        })
    });
    let [Some(downsample), Some(blur_h), Some(blur_v)] =
        pipelines.map(|id| p.pipeline_cache.get_render_pipeline(id))
    else {
        return; // still compiling
    };

    if p.res.scratch.as_ref().is_none_or(|(s, ..)| *s != size) {
        let texture = p.device.create_texture(&TextureDescriptor {
            label: Some("blur_scratch"),
            size: Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        p.res.scratch = Some((size, texture, view));
    }
    let sigma = (p.blur.sigma / DOWNSCALE as f32).min(MAX_SIGMA);
    p.res.params.set(Vec4::new(sigma, 0.0, 0.0, 0.0));
    p.res.params.write_buffer(&p.device, &p.queue);
    let (Some((_, _, scratch)), Some(params)) = (&p.res.scratch, p.res.params.binding()) else {
        return;
    };

    let layout = p.pipeline_cache.get_bind_group_layout(&p.res.layout);
    let passes = [
        (
            downsample,
            view_target.main_texture_view(),
            &image.texture_view,
        ),
        (blur_h, &image.texture_view, scratch),
        (blur_v, scratch, &image.texture_view),
    ];
    for (pipeline, source, destination) in passes {
        let bind_group = p.device.create_bind_group(
            "blur_bind_group",
            &layout,
            &BindGroupEntries::sequential((source, &p.res.sampler, params.clone())),
        );
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("blur_pass"),
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
            });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
