//! Offscreen rendering of a composed shader on one wgpu adapter.
//!
//! The same pass structure as the app and its `--render` CLI: buffer passes
//! `buffer_a`..`buffer_d` into ping-pong `Rgba32Float` targets, then the
//! image pass, here into an `Rgba8UnormSrgb` target (the app's swapchain
//! format) that is read back. Every adapter gets the same uniforms, so any
//! pixel that differs comes from the shader's path through that backend.

use std::collections::HashMap;

use wesl_shadertoy::compile::{self, CompiledShader};

use crate::image::Image;

/// What a render sees: canvas size, clock and mouse, identical on every
/// adapter.
#[derive(Clone, Copy, Debug)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    /// `i_time()` of the frame that is read back.
    pub time: f32,
    /// Frames replayed up to `time` when the shader has buffer passes (one
    /// frame otherwise), so feedback loops get to run a little.
    pub buffer_frames: u32,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            width: 256,
            height: 144,
            time: 3.0,
            buffer_frames: 8,
        }
    }
}

/// Fullscreen triangle, as in the app's headless renderer.
const VERTEX_WGSL: &str = r#"
struct VOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0),
    );
    let flip = vec2<f32>(1.0, -1.0);
    var out: VOut;
    out.position = vec4<f32>(corners[index] * flip, 0.0, 1.0);
    out.uv = corners[index] * flip * 0.5 + 0.5;
    return out;
}
"#;

const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const BUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// A device on one adapter, plus the pipeline-independent resources every
/// render shares.
pub struct Gpu {
    /// Short, file-name-safe name: the backend (`vulkan`, `gl`), suffixed
    /// when a backend has several adapters.
    pub label: String,
    /// Adapter, driver and backend, for reports.
    pub description: String,
    pub backend: wgpu::Backend,
    /// Whether `Rgba32Float` is renderable, i.e. buffer passes can run.
    pub float_targets: bool,
    device: wgpu::Device,
    queue: wgpu::Queue,
    vs_module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    dummy_view: wgpu::TextureView,
    /// `i_ui` (transparent: there is no UI here) and its sampler.
    ui_view: wgpu::TextureView,
    ui_sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    uniforms: UniformLayout,
}

impl Gpu {
    /// A device on every adapter wgpu finds for `backends`, Vulkan first (the
    /// reference the others are compared against). Adapters whose device
    /// request fails come back as messages instead.
    pub fn all(backends: wgpu::Backends) -> (Vec<Gpu>, Vec<String>) {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        let instance = wgpu::Instance::new(desc);
        let mut adapters = pollster::block_on(instance.enumerate_adapters(backends));
        adapters.sort_by_key(|a| a.get_info().backend != wgpu::Backend::Vulkan);

        let (mut gpus, mut skipped) = (Vec::<Gpu>::new(), Vec::new());
        for adapter in adapters {
            let info = adapter.get_info();
            let driver = [info.backend.to_str(), &info.driver]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let description = format!("{} ({driver})", info.name);
            let mut label = info.backend.to_str().to_string();
            let same = gpus.iter().filter(|g| g.backend == info.backend).count();
            if same > 0 {
                label = format!("{label}{}", same + 1);
            }
            match pollster::block_on(Gpu::new(adapter, label, description.clone())) {
                Ok(gpu) => gpus.push(gpu),
                Err(e) => skipped.push(format!("{description}: {e}")),
            }
        }
        (gpus, skipped)
    }

    async fn new(
        adapter: wgpu::Adapter,
        label: String,
        description: String,
    ) -> Result<Gpu, String> {
        let backend = adapter.get_info().backend;
        let float_targets = adapter
            .get_texture_format_features(BUFFER_FORMAT)
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("backend-diff"),
                // GL adapters fall short of WebGPU's default limits.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen-vs"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });
        let uniforms = UniformLayout::new();

        // Bindings 1..8 of the composed module: sampler, uniforms, channels,
        // and `i_ui` with its sampler (always a transparent dummy here).
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadertoy-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(uniforms.size),
                    },
                    count: None,
                },
                tex(3),
                tex(4),
                tex(5),
                tex(6),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadertoy-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let dummy_view = device
            .create_texture(&texture_descriptor("channel-dummy", 1, 1, BUFFER_FORMAT))
            .create_view(&Default::default());
        // Textures start zeroed: a transparent `i_ui`.
        let ui_view = device
            .create_texture(&texture_descriptor(
                "ui-dummy",
                1,
                1,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ))
            .create_view(&Default::default());
        let ui_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadertoy-uniform"),
            size: uniforms.size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Gpu {
            label,
            description,
            backend,
            float_targets,
            device,
            queue,
            vs_module,
            bgl,
            layout,
            sampler,
            dummy_view,
            ui_view,
            ui_sampler,
            uniform,
            uniforms,
        })
    }

    /// Render `shader` in `scene` and read back the final image. wgpu
    /// validation errors (including a backend's own shader compiler
    /// rejecting the translated code) come back as `Err`.
    pub fn render(&self, shader: &CompiledShader, scene: &Scene) -> Result<Image, String> {
        if shader.buffers != 0 && !self.float_targets {
            return Err("Rgba32Float is not renderable; buffer passes cannot run".into());
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let image = self.render_unscoped(shader, scene);
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(error.to_string());
        }
        image
    }

    fn render_unscoped(&self, shader: &CompiledShader, scene: &Scene) -> Result<Image, String> {
        let device = &self.device;
        let (width, height) = (scene.width, scene.height);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("user"),
            source: wgpu::ShaderSource::Wgsl(shader.wgsl.as_str().into()),
        });
        let pipeline = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.vs_module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let defined = |i: usize| shader.buffers & (1 << i) != 0;
        let main_pipeline = pipeline(compile::MAIN_ENTRY, IMAGE_FORMAT);
        let buffer_pipelines: Vec<_> = (0..4)
            .filter(|&i| defined(i))
            .map(|i| {
                let entry = compile::buffer_entry(compile::BUFFER_NAMES[i]);
                (i, pipeline(&entry, BUFFER_FORMAT))
            })
            .collect();

        // Ping-pong pairs for the defined channels.
        let pairs: Vec<[wgpu::TextureView; 2]> = (0..4)
            .map(|i| {
                std::array::from_fn(|slot| {
                    if !defined(i) {
                        return self.dummy_view.clone();
                    }
                    let label = format!("{}_{slot}", compile::BUFFER_NAMES[i]);
                    device
                        .create_texture(&texture_descriptor(&label, width, height, BUFFER_FORMAT))
                        .create_view(&Default::default())
                })
            })
            .collect();
        let target =
            device.create_texture(&texture_descriptor("image", width, height, IMAGE_FORMAT));
        let target_view = target.create_view(&Default::default());

        let bind_group = |channels: [&wgpu::TextureView; 4]| {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform.as_entire_binding(),
                },
            ];
            for (i, view) in channels.into_iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: 3 + i as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
            entries.push(wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&self.ui_view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(&self.ui_sampler),
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.bgl,
                entries: &entries,
            })
        };
        let draw = |encoder: &mut wgpu::CommandEncoder,
                    view: &wgpu::TextureView,
                    pipeline: &wgpu::RenderPipeline,
                    bind_group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        };

        let frames = if shader.buffers != 0 {
            scene.buffer_frames.max(1)
        } else {
            1
        };
        for frame in 0..frames {
            let t = scene.time * (frame + 1) as f32 / frames as f32;
            let dt = scene.time / frames as f32;
            let bytes = self.uniforms.bytes(shader, scene, frame, t, dt);
            self.queue.write_buffer(&self.uniform, 0, &bytes);
            let (write, read) = ((frame % 2) as usize, (frame % 2) as usize ^ 1);
            let mut encoder = device.create_command_encoder(&Default::default());
            // Buffer passes, A→D: earlier channels read this frame's result,
            // self and later ones the previous frame's (as in the app).
            for (i, pipeline) in &buffer_pipelines {
                let reads = std::array::from_fn(|j| &pairs[j][if j < *i { write } else { read }]);
                draw(
                    &mut encoder,
                    &pairs[*i][write],
                    pipeline,
                    &bind_group(reads),
                );
            }
            let reads = std::array::from_fn(|j| &pairs[j][write]);
            draw(
                &mut encoder,
                &target_view,
                &main_pipeline,
                &bind_group(reads),
            );
            self.queue.submit([encoder.finish()]);
        }
        self.read_back(&target, width, height)
    }

    fn read_back(&self, target: &wgpu::Texture, width: u32, height: u32) -> Result<Image, String> {
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            target.size(),
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        receiver
            .recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let row = width as usize * 4;
        let rgba = mapped
            .chunks(stride as usize)
            .flat_map(|line| &line[..row])
            .copied()
            .collect();
        Ok(Image {
            width,
            height,
            rgba,
        })
    }
}

fn texture_descriptor(
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    }
}

/// The `ShaderToyUniforms` block, laid out by naga from the app's own WGSL
/// declaration ([`compile::SHADERTOY_MODULE`]), so the bytes written here
/// follow the struct without depending on the app's Bevy-side twin.
struct UniformLayout {
    size: u64,
    /// Member name → (byte offset, array stride or 0).
    members: HashMap<String, (u32, u32)>,
}

impl UniformLayout {
    fn new() -> Self {
        let module = naga::front::wgsl::parse_str(compile::SHADERTOY_MODULE)
            .expect("the shadertoy module parses");
        let (handle, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ShaderToyUniforms"))
            .expect("ShaderToyUniforms is declared");
        let naga::TypeInner::Struct { members, .. } = &ty.inner else {
            panic!("ShaderToyUniforms is a struct");
        };
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).expect("layout");
        let members = members
            .iter()
            .map(|m| {
                let stride = match module.types[m.ty].inner {
                    naga::TypeInner::Array { stride, .. } => stride,
                    _ => 0,
                };
                (m.name.clone().unwrap_or_default(), (m.offset, stride))
            })
            .collect();
        Self {
            size: u64::from(layouter[handle].size),
            members,
        }
    }

    /// The uniform bytes of frame `frame` at time `t`: the scene's canvas, a
    /// fixed date and a mouse held down at a fixed spot (so mouse-driven
    /// code paths run too), and every tweakable constant at its source value.
    fn bytes(
        &self,
        shader: &CompiledShader,
        scene: &Scene,
        frame: u32,
        t: f32,
        dt: f32,
    ) -> Vec<u8> {
        let mut out = vec![0u8; self.size as usize];
        let mut put = |member: &str, index: u32, words: &[u32]| {
            let (offset, stride) = self.members[member];
            let at = (offset + index * stride) as usize;
            for (k, w) in words.iter().enumerate() {
                out[at + 4 * k..at + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
            }
        };
        let f = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        let (w, h) = (scene.width as f32, scene.height as f32);
        put("i_time", 0, &f(&[t]));
        put("i_time_delta", 0, &f(&[dt]));
        put("i_frame", 0, &[frame]);
        put("i_resolution", 0, &f(&[w, h, 1.0, 0.0]));
        put("i_mouse", 0, &f(&[0.6 * w, 0.4 * h, 0.6 * w, 0.4 * h]));
        // Noon on 25 October 2026 (months count from 0).
        put("i_date", 0, &f(&[2026.0, 9.0, 25.0, 43_200.0]));
        put("i_sample_rate", 0, &f(&[44_100.0]));
        put("i_frame_rate", 0, &f(&[60.0]));
        let defined = |i: u32| shader.buffers & (1 << i) != 0;
        let channel_time = (0..4).map(|i| if defined(i) { t } else { 0.0 });
        put("i_channel_time", 0, &f(&channel_time.collect::<Vec<_>>()));
        for i in (0..4).filter(|&i| defined(i)) {
            put("i_channel_resolution", i, &f(&[w, h, 0.0, 0.0]));
        }
        for (slot, c) in shader.tweaks.iter().enumerate() {
            put("i_tweaks", slot as u32, &f(&c.value));
        }
        out
    }
}
