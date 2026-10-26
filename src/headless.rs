//! Native-only offscreen renderer: `wesl-shadertoy --render <example|file> <out.png> [time]`.
//!
//! Creates a windowless wgpu device, compiles an example (or `.wesl` file)
//! through the exact same [`crate::compile::compile_user_shader`] path the app
//! uses, renders frames offscreen, and writes a PNG. No display server is
//! involved, which makes it usable as a deterministic smoke test for examples
//! and ports. Buffer passes (`buffer_a`..`buffer_d`) run with the same
//! ping-pong semantics as the app; `--frames N` replays N frames so feedback
//! loops accumulate; `--heat [--heat-max N]` renders the cost heatmap
//! ([`crate::heatmap`]) instead of the image.

use crate::compile::{self, compile_user_shader_with};
use crate::examples;
use crate::heatmap;
use crate::render::{ShaderToyUniform, uniform_bytes};
use crate::tweak::MAX_TWEAKS;
use bevy::math::Vec4;
use bevy::render::render_resource::ShaderType;

/// Fullscreen triangle feeding the composed `package::main` module. The uv
/// attribute mirrors Bevy's fullscreen vertex shader (v = 0 at the top).
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
    // NDC y = +1 is the bottom of the framebuffer (wgpu is top-left), so
    // uv.y = 0 lands on the top row, like Bevy's fullscreen pass.
    let flip = vec2<f32>(1.0, -1.0);
    var out: VOut;
    out.position = vec4<f32>(corners[index] * flip, 0.0, 1.0);
    out.uv = corners[index] * flip * 0.5 + 0.5;
    return out;
}
"#;

pub fn run_render_cli(args: &[String]) {
    let mut iter = args.iter().skip(1);
    let mut time = 3.0f32;
    let mut frames = 1u32;
    let mut debugger = false;
    let mut heat = false;
    let mut heat_max = heatmap::DEFAULT_MAX;
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--render" => {}
            "--debugger" => debugger = true,
            "--heat" => heat = true,
            "--heat-max" => {
                if let Some(m) = iter.next() {
                    heat_max = m.parse().unwrap_or(heatmap::DEFAULT_MAX);
                }
            }
            "--time" => {
                if let Some(t) = iter.next() {
                    time = t.parse().unwrap_or(3.0);
                }
            }
            "--frames" => {
                if let Some(f) = iter.next() {
                    frames = f.parse().unwrap_or(1).max(1);
                }
            }
            _ => positional.push(arg.clone()),
        }
    }
    if positional.len() < 2 {
        eprintln!(
            "usage: wesl-shadertoy --render <example-name|file.wesl> <out.png> \
             [--time SECS] [--frames N] [--debugger] [--heat [--heat-max N]]"
        );
        std::process::exit(2);
    }
    let (name, source) = resolve_source(&positional[0]);
    let out = positional[1].clone();

    let compiled = match compile_user_shader_with(&source, heat) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{name}: compile failed:\n{e}");
            std::process::exit(1);
        }
    };

    let (width, height) = (640, 360);
    let result = pollster::block_on(render(
        &compiled, width, height, time, frames, debugger, heat_max,
    ));
    match result {
        Ok(rgba) => {
            if let Err(e) = write_png(&out, width, height, &rgba) {
                eprintln!("{name}: writing {out} failed: {e}");
                std::process::exit(1);
            }
            println!("{name}: wrote {out}");
        }
        Err(e) => {
            eprintln!("{name}: render failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Example name (case-insensitive) or path to a `.wesl` file.
pub fn resolve_source(arg: &str) -> (String, String) {
    if let Some(e) = examples::by_name(arg) {
        return (e.name.to_string(), e.code.to_string());
    }
    match std::fs::read_to_string(arg) {
        Ok(text) => (arg.to_string(), text),
        Err(e) => {
            eprintln!("no example named {arg:?} and no readable file ({e})");
            std::process::exit(2);
        }
    }
}

/// One ping-pong slot for a buffer channel.
struct Surface {
    view: wgpu::TextureView,
}

/// The debugger's blit pass: the image target plus the overlay → output.
struct DebuggerBlit {
    image_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
}

/// A compiled shader on a windowless device, ready to render frames at a
/// fixed size and read the result back. `--render` uses one once;
/// [`crate::drag`] keeps one per compile and renders many variants of the
/// same shader with different `i_tweaks`.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    buffers: u8,
    bgl: wgpu::BindGroupLayout,
    main_pipeline: wgpu::RenderPipeline,
    buffer_pipelines: Vec<(usize, wgpu::RenderPipeline)>,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    dummy_view: wgpu::TextureView,
    /// Transparent 1x1 `i_ui` (no UI is rendered here) and its sampler.
    ui_view: wgpu::TextureView,
    ui_sampler: wgpu::Sampler,
    pairs: Vec<[Surface; 2]>,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    debugger: Option<DebuggerBlit>,
    /// The heatmap's scale, read only by a heat-mode compile.
    pub heat_max: f32,
}

/// Render `frames` frames of `compiled` (constants at their source values)
/// and read back the final image as RGBA8.
/// With `debugger` on, the overlay is composited and `iMouse` is parked at
/// the centre so the crosshair and probe boxes are visible in the PNG.
/// `heat_max` is the heatmap's scale (used only by a heat-mode compile).
async fn render(
    compiled: &compile::CompiledShader,
    width: u32,
    height: u32,
    time: f32,
    frames: u32,
    debugger: bool,
    heat_max: f32,
) -> Result<Vec<u8>, String> {
    let mut renderer = Renderer::new(
        &compiled.wgsl,
        compiled.buffers,
        width,
        height,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        debugger,
    )
    .await?;
    renderer.heat_max = heat_max;
    let mut tweaks = [Vec4::ZERO; MAX_TWEAKS];
    for (slot, c) in compiled.tweaks.iter().enumerate() {
        tweaks[slot] = Vec4::from_array(c.value);
    }
    // Park iMouse at the centre (y-up, like the app) with a held click
    // there too, so the crosshair, ring and probe boxes show.
    let (mx, my) = (width as f32 / 2.0, height as f32 / 2.0);
    let mouse = if debugger {
        Vec4::new(mx, my, mx, my)
    } else {
        Vec4::ZERO
    };
    renderer.render(time, frames, &tweaks, mouse)?;
    renderer.read()
}

impl Renderer {
    /// Set up a device and pipelines for the composed module `wgsl`.
    /// `format` is the output's: `Rgba8UnormSrgb` for PNGs, `Rgba32Float`
    /// for unquantized readback. With `debugger` on, the main pass renders
    /// into the offscreen image target and a blit pass composites the
    /// overlay, like the app; otherwise the main pass renders straight to
    /// the output.
    pub async fn new(
        wgsl: &str,
        buffers: u8,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        debugger: bool,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|e| e.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| e.to_string())?;
        // A shader the GPU rejects (naga passed it, the backend did not)
        // comes back as `Err` instead of wgpu's default handler panicking,
        // which with `panic = "abort"` would take the whole app down from
        // Drag mode's estimation thread.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let user_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("user"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen-vs"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });

        // The composed module references bindings 1..8 (sampler, uniform,
        // four channels, the UI texture and its sampler); binding 0 (the
        // app's view texture) is never sampled, so this layout skips it.
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                // Channels are Rgba32Float (unfilterable in core WebGPU).
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadertoy-bgl"),
            entries: &[
                tex(3),
                tex(4),
                tex(5),
                tex(6),
                // `i_ui`: there is no UI here, so always the transparent dummy.
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    // Must match the app's layout (fullscreen_material.rs).
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(ShaderToyUniform::min_size()),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadertoy-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let make_pipeline = |entry: &str, format: wgpu::TextureFormat, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs_module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &user_module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let main_format = if debugger {
            crate::debugger::IMAGE_FORMAT
        } else {
            format
        };
        let main_pipeline = make_pipeline(compile::MAIN_ENTRY, main_format, "main-rp");
        let buffer_pipelines: Vec<_> = compile::BUFFER_NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| buffers & (1 << i) != 0)
            .map(|(i, name)| {
                (
                    i,
                    make_pipeline(
                        &compile::buffer_entry(name),
                        wgpu::TextureFormat::Rgba32Float,
                        "buffer-rp",
                    ),
                )
            })
            .collect();

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadertoy-uniform"),
            size: ShaderToyUniform::min_size().get(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

        // Black 1x1 stand-in for undefined channels (and binding-0 filler).
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("channel-dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());

        let ui_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ui-dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let ui_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Ping-pong pairs for the defined buffers, viewport-sized like the app.
        let make_surface = |label: &str| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            Surface {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            }
        };
        let mut pairs: Vec<[Surface; 2]> = Vec::new();
        for (i, name) in compile::BUFFER_NAMES.iter().enumerate() {
            if buffers & (1 << i) != 0 {
                pairs.push([
                    make_surface(&format!("buffer_{name}_0")),
                    make_surface(&format!("buffer_{name}_1")),
                ]);
            } else {
                pairs.push([
                    Surface {
                        view: dummy_view.clone(),
                    },
                    Surface {
                        view: dummy_view.clone(),
                    },
                ]);
            }
        }

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("smoke-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let debugger = debugger.then(|| {
            DebuggerBlit::new(
                &device,
                &queue,
                &vs_module,
                &uniform,
                (width, height),
                format,
            )
        });
        if let Some(error) = scope.pop().await {
            return Err(error.to_string());
        }

        Ok(Self {
            device,
            queue,
            width,
            height,
            format,
            buffers,
            bgl,
            main_pipeline,
            buffer_pipelines,
            uniform,
            sampler,
            dummy_view,
            ui_view,
            ui_sampler,
            pairs,
            target,
            view,
            debugger,
            heat_max: heatmap::DEFAULT_MAX,
        })
    }

    fn bind_group(&self, label: &str, channels: [&wgpu::TextureView; 4]) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(channels[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(channels[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(channels[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(channels[3]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self.ui_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.ui_sampler),
                },
            ],
        })
    }

    /// Render `frames` frames (buffer passes, the main pass, then the
    /// debugger blit pass if on), ending at `time`, with the given
    /// `i_tweaks` and `i_mouse`. Every call counts frames from 0; the
    /// buffers keep what the previous call left in them.
    pub fn render(
        &mut self,
        time: f32,
        frames: u32,
        tweaks: &[Vec4; MAX_TWEAKS],
        mouse: Vec4,
    ) -> Result<(), String> {
        let (width, height, buffers) = (self.width, self.height, self.buffers);
        let (dummy_view, pairs) = (&self.dummy_view, &self.pairs);
        for frame in 0..frames {
            let t = time * (frame + 1) as f32 / frames as f32;
            let mut uniforms = ShaderToyUniform {
                time: t,
                time_delta: time / frames as f32,
                frame, // 0 on the first frame, like the app
                resolution: Vec4::new(width as f32, height as f32, 1.0, 0.0),
                mouse,
                sample_rate: 44100.0,
                frame_rate: 60.0,
                heat_max: self.heat_max,
                tweaks: *tweaks,
                ..Default::default()
            };
            for i in 0..4 {
                if buffers & (1 << i) != 0 {
                    // iChannelTime counts from (re)compile, which is t = 0 here.
                    uniforms.channel_time[i] = t;
                    uniforms.channel_resolution[i] =
                        Vec4::new(width as f32, height as f32, 0.0, 0.0);
                }
            }
            let bytes = uniform_bytes(&uniforms);
            self.queue.write_buffer(&self.uniform, 0, &bytes);

            let write_slot = (frame % 2) as usize;
            let read_slot = write_slot ^ 1;

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("smoke-encoder"),
                });

            // Buffer passes, A→D. Earlier channels read this frame's result;
            // self and later channels read the previous frame's slot.
            for &(i, ref pipeline) in &self.buffer_pipelines {
                let reads: [&wgpu::TextureView; 4] = std::array::from_fn(|j| {
                    if buffers & (1 << j) == 0 {
                        dummy_view
                    } else if j < i {
                        &pairs[j][write_slot].view
                    } else {
                        &pairs[j][read_slot].view
                    }
                });
                let bind_group = self.bind_group("buffer-bg", reads);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("buffer-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &pairs[i][write_slot].view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
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

            // Main pass: every defined channel reads this frame's fresh result.
            let channels: [&wgpu::TextureView; 4] = std::array::from_fn(|j| {
                if buffers & (1 << j) == 0 {
                    dummy_view
                } else {
                    &pairs[j][write_slot].view
                }
            });
            let bind_group = self.bind_group("main-bg", channels);
            {
                // Like the app, the main pass renders into the offscreen
                // image target only while the debugger is on.
                let target = match &self.debugger {
                    Some(blit) => &blit.image_view,
                    None => &self.view,
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("smoke-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.main_pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }

            // Blit pass: image target plus the debugger overlay -> final target.
            if let Some(blit) = &self.debugger {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("debugger-blit-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&blit.pipeline);
                pass.set_bind_group(0, &blit.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }

            self.queue.submit(std::iter::once(encoder.finish()));
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Read back the last rendered frame: tightly packed rows, top row
    /// first, in the output format (4 bytes per pixel for RGBA8, 16 for
    /// RGBA32F).
    pub fn read(&self) -> Result<Vec<u8>, String> {
        let (width, height) = (self.width, self.height);
        let pixel = self
            .format
            .block_copy_size(None)
            .ok_or("output format has no fixed pixel size")?;
        let row = width * pixel;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback-encoder"),
            });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("smoke-readback"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

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
        let (stride, row) = (stride as usize, row as usize);
        let mut out = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            out.extend_from_slice(&mapped[y * stride..y * stride + row]);
        }
        Ok(out)
    }
}

impl DebuggerBlit {
    /// The font atlas, the offscreen image target and the blit pipeline
    /// that composites them into an output of `format`.
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vs_module: &wgpu::ShaderModule,
        uniform: &wgpu::Buffer,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Self {
        let debugger_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debugger"),
            source: wgpu::ShaderSource::Wgsl(crate::debugger::DEBUGGER_WGSL.into()),
        });
        let atlas = crate::debugger::font_atlas_pixels();
        let font = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("debugger-font-atlas"),
            size: wgpu::Extent3d {
                width: crate::debugger::ATLAS_SIZE,
                height: crate::debugger::ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            font.as_image_copy(),
            &atlas,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(crate::debugger::ATLAS_SIZE), // R8, multiple of 256
                rows_per_image: Some(crate::debugger::ATLAS_SIZE),
            },
            wgpu::Extent3d {
                width: crate::debugger::ATLAS_SIZE,
                height: crate::debugger::ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
        let font_view = font.create_view(&wgpu::TextureViewDescriptor::default());

        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::debugger::IMAGE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let image_view = image.create_view(&wgpu::TextureViewDescriptor::default());

        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debugger-blit-bgl"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(ShaderToyUniform::min_size()),
                    },
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debugger-blit-pl"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debugger-blit-rp"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: vs_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &debugger_module,
                entry_point: Some(crate::debugger::BLIT_ENTRY),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debugger-blit-bg"),
            layout: &blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&font_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        Self {
            image_view,
            pipeline,
            bind_group,
        }
    }
}

pub fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}
