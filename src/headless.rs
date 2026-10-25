//! Native-only offscreen renderer: `wesl-shadertoy --render <preset|file> <out.png> [time]`.
//!
//! Creates a windowless wgpu device, compiles a preset (or `.wgsl` file)
//! through the exact same [`crate::compile::compile_user_shader`] path the app
//! uses, renders frames offscreen, and writes a PNG. No display server is
//! involved, which makes it usable as a deterministic smoke test for presets
//! and ports. Buffer passes (`buffer_a`..`buffer_d`) run with the same
//! ping-pong semantics as the app; `--frames N` replays N frames so feedback
//! loops accumulate.

use crate::compile::{self, compile_user_shader};
use crate::presets;
use crate::render::{ShaderToyUniform, uniform_bytes};
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
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--render" => {}
            "--debugger" => debugger = true,
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
            "usage: wesl-shadertoy --render <preset-name|file.wgsl> <out.png> \
             [--time SECS] [--frames N] [--debugger]"
        );
        std::process::exit(2);
    }
    let (name, source) = resolve_source(&positional[0]);
    let out = positional[1].clone();

    let compiled = match compile_user_shader(&source) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{name}: compile failed:\n{e}");
            std::process::exit(1);
        }
    };

    let (width, height) = (640, 360);
    let result = pollster::block_on(render(&compiled, width, height, time, frames, debugger));
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

/// Preset name (case-insensitive) or path to a `.wgsl` file.
fn resolve_source(arg: &str) -> (String, String) {
    if let Some(p) = presets::ALL
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(arg))
    {
        return (p.name.to_string(), p.code.to_string());
    }
    match std::fs::read_to_string(arg) {
        Ok(text) => (arg.to_string(), text),
        Err(e) => {
            eprintln!("no preset named {arg:?} and no readable file ({e})");
            std::process::exit(2);
        }
    }
}

/// One ping-pong slot for a buffer channel.
struct Surface {
    view: wgpu::TextureView,
}

/// Render `frames` frames (buffer passes, the main pass into the offscreen
/// image target, then the debugger blit pass; with `debugger` off the main
/// pass renders straight to the output) and read back the final image.
/// With `debugger` on, the overlay is composited and `iMouse` is parked at
/// the centre so the crosshair and probe boxes are visible in the PNG.
async fn render(
    compiled: &compile::CompiledShader,
    width: u32,
    height: u32,
    time: f32,
    frames: u32,
    debugger: bool,
) -> Result<Vec<u8>, String> {
    let (wgsl, buffers) = (compiled.wgsl.as_str(), compiled.buffers);
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| e.to_string())?;

    let user_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("user"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fullscreen-vs"),
        source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
    });
    let debugger_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("debugger"),
        source: wgpu::ShaderSource::Wgsl(crate::debugger::DEBUGGER_WGSL.into()),
    });

    // The composed module references bindings 1..6 (sampler, uniform, four
    // channels); binding 0 (the app's view texture) is never sampled, so this
    // layout skips it.
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

    // Like the app, the main pass renders into the offscreen image target
    // only while the debugger is on, and straight to the output otherwise.
    let main_format = if debugger {
        crate::debugger::IMAGE_FORMAT
    } else {
        wgpu::TextureFormat::Rgba8UnormSrgb
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // --- debugger plumbing: font atlas, offscreen image target, blit pass ---
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

    let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("debugger-blit-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
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
    let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("debugger-blit-pl"),
        bind_group_layouts: &[Some(&blit_bgl)],
        immediate_size: 0,
    });
    let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("debugger-blit-rp"),
        layout: Some(&blit_layout),
        vertex: wgpu::VertexState {
            module: &vs_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &debugger_module,
            entry_point: Some(crate::debugger::BLIT_ENTRY),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
    let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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

    let make_bind_group = |label: &str, channels: [&wgpu::TextureView; 4]| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
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
            ],
        })
    };

    // Replay `frames` frames, ending at the requested `time`.
    for frame in 0..frames {
        let t = time * (frame + 1) as f32 / frames as f32;
        let mut uniforms = ShaderToyUniform {
            time: t,
            time_delta: time / frames as f32,
            frame, // 0 on the first frame, like the app
            resolution: Vec4::new(width as f32, height as f32, 1.0, 0.0),
            sample_rate: 44100.0,
            frame_rate: 60.0,
            ..Default::default()
        };
        // The constants at their source values, as the app starts them.
        for (slot, c) in compiled.tweaks.iter().enumerate() {
            uniforms.tweaks[slot] = Vec4::from_array(c.value);
        }
        if debugger {
            // Park iMouse at the centre (y-up, like the app) with a held
            // click there too, so the crosshair, ring and probe boxes show.
            let (mx, my) = (width as f32 / 2.0, height as f32 / 2.0);
            uniforms.mouse = Vec4::new(mx, my, mx, my);
        }
        for i in 0..4 {
            if buffers & (1 << i) != 0 {
                // iChannelTime counts from (re)compile, which is t = 0 here.
                uniforms.channel_time[i] = t;
                uniforms.channel_resolution[i] = Vec4::new(width as f32, height as f32, 0.0, 0.0);
            }
        }
        let bytes = uniform_bytes(&uniforms);
        queue.write_buffer(&uniform, 0, &bytes);

        let write_slot = (frame % 2) as usize;
        let read_slot = write_slot ^ 1;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("smoke-encoder"),
        });

        // Buffer passes, A→D. Earlier channels read this frame's result;
        // self and later channels read the previous frame's slot.
        for &(i, ref pipeline) in &buffer_pipelines {
            let reads: [&wgpu::TextureView; 4] = std::array::from_fn(|j| {
                if buffers & (1 << j) == 0 {
                    &dummy_view
                } else if j < i {
                    &pairs[j][write_slot].view
                } else {
                    &pairs[j][read_slot].view
                }
            });
            let bind_group = make_bind_group("buffer-bg", reads);
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
                &dummy_view
            } else {
                &pairs[j][write_slot].view
            }
        });
        let bind_group = make_bind_group("main-bg", channels);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("smoke-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: if debugger { &image_view } else { &view },
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
            pass.set_pipeline(&main_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Blit pass: image target plus the debugger overlay -> final target.
        if debugger {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debugger-blit-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(&blit_pipeline);
            pass.set_bind_group(0, &blit_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
    }

    // Read back the final frame.
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback-encoder"),
    });
    let readback_bytes = (width * 4).next_multiple_of(256) * height;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke-readback"),
        size: u64::from(readback_bytes),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((width * 4).next_multiple_of(256)),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = sender.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    receiver
        .recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let stride = (width * 4).next_multiple_of(256) as usize;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height as usize {
        rgba.extend_from_slice(&mapped[row * stride..row * stride + width as usize * 4]);
    }
    Ok(rgba)
}

fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}
