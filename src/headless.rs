//! Native-only offscreen renderer: `wesl-shadertoy --render <preset|file> <out.png> [time]`.
//!
//! Creates a windowless wgpu device, compiles a preset (or `.wesl` file)
//! through the exact same [`crate::compile::compile_user_shader`] path the app
//! uses, renders one frame, and writes a PNG. No display server is involved,
//! which makes it usable as a deterministic smoke test for presets and ports.

use crate::compile::compile_user_shader;
use crate::presets;

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
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--render" => {}
            "--time" => {
                if let Some(t) = iter.next() {
                    time = t.parse().unwrap_or(3.0);
                }
            }
            _ => positional.push(arg.clone()),
        }
    }
    if positional.len() < 2 {
        eprintln!("usage: wesl-shadertoy --render <preset-name|file.wesl> <out.png> [--time SECS]");
        std::process::exit(2);
    }
    let (name, source) = resolve_source(&positional[0]);
    let out = positional[1].clone();

    let wgsl = match compile_user_shader(&source) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{name}: compile failed:\n{e}");
            std::process::exit(1);
        }
    };

    match pollster::block_on(render_one_frame(&wgsl, 640, 360, time)) {
        Ok(rgba) => {
            if let Err(e) = write_png(&out, 640, 360, &rgba) {
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

/// Preset name (case-insensitive) or path to a `.wesl` file.
fn resolve_source(arg: &str) -> (String, String) {
    if let Some(p) = presets::ALL.iter().find(|p| p.name.eq_ignore_ascii_case(arg)) {
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

async fn render_one_frame(wgsl: &str, width: u32, height: u32, time: f32) -> Result<Vec<u8>, String> {
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

    // The composed module only references @group(0) @binding(2) (the uniform);
    // bindings 0/1 (view texture + sampler) exist in the app's pipeline but
    // the WGSL never touches them, so this layout can omit them.
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadertoy-uniform-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(160),
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadertoy-pl"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadertoy-rp"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &vs_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &user_module,
            entry_point: Some("fragment"),
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

    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shadertoy-uniform"),
        size: 160,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Field order mirrors ShaderToyUniform (see render/mod.rs); resolution is
    // the only field a smoke render really needs, plus the clock.
    let mut bytes = [0u8; 160];
    bytes[0..4].copy_from_slice(&time.to_le_bytes());
    bytes[4..8].copy_from_slice(&0.016f32.to_le_bytes());
    bytes[16..20].copy_from_slice(&(width as f32).to_le_bytes());
    bytes[20..24].copy_from_slice(&(height as f32).to_le_bytes());
    bytes[24..28].copy_from_slice(&1.0f32.to_le_bytes());
    bytes[64..68].copy_from_slice(&44100.0f32.to_le_bytes());
    queue.write_buffer(&uniform, 0, &bytes);

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadertoy-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 2,
            resource: uniform.as_entire_binding(),
        }],
    });

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

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("smoke-encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("smoke-pass"),
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
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
    let mapped = slice
        .get_mapped_range()
        .map_err(|e| e.to_string())?;
    let stride = (width * 4).next_multiple_of(256) as usize;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height as usize {
        rgba.extend_from_slice(&mapped[row * stride..row * stride + width as usize * 4]);
    }
    Ok(rgba)
}

fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file =
        std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}
