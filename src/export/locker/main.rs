//! A lock screen that draws a shader exported from
//! [wesl-shadertoy](https://github.com/barafael/wesl-shadertoy), for niri and
//! the other compositors with `ext-session-lock-v1` (sway, Hyprland, …).
//!
//! It locks the session at once and covers every output with the shader.
//! Typing shows a row of dots over it; Enter checks the password with PAM
//! (in a thread, so the shader keeps running) and unlocks on success.
//! Escape or Ctrl+U clears what was typed. What PAM says (an account locked
//! after failed attempts, a password about to expire) shows under the dots,
//! in the system's sans-serif font as fontconfig names it.
//!
//! The compositor, not this program, keeps the session locked: if it dies
//! (a crash, a GPU reset, a killed process), the screen stays locked and
//! blank until another locker unlocks it. Nothing here can unlock the
//! session except a password PAM accepts.

mod config;

use std::ffi::{CStr, CString};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use pam_client::{ConversationHandler, ErrorCode, Flag};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_session_lock,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, channel,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_surface},
};
use zeroize::Zeroizing;

/// Words in the shader's uniform block (`ShaderToyUniforms` in
/// `shader.wgsl`): ten `vec4`s of clock, resolution, mouse, date and channel
/// data, then the constants.
const UNIFORM_WORDS: usize = 40 + 4 * config::TWEAKS.len();

/// Words in the indicator's uniform block (`Indicator` in `indicator.wgsl`).
const INDICATOR_WORDS: usize = 12;

/// How long the dots stay up after the last key or PAM's answer, and the
/// red after a wrong password.
const SHOW_FOR: Duration = Duration::from_secs(5);
/// How long they stay up when PAM had something to say, to read it.
const SHOW_MESSAGES_FOR: Duration = Duration::from_secs(10);
const WRONG_FOR: Duration = Duration::from_secs(2);

const USAGE: &str = "\
Locks the session and draws the shader until the user's password is entered.

Usage: [--fps N] [--pam-service NAME]

  --fps N              Frames per second at most; 0 draws at the display's rate.
  --pam-service NAME   The PAM service that checks the password (default: login).
";

struct Args {
    fps: u32,
    pam_service: String,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        fps: config::DEFAULT_FPS,
        pam_service: "login".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--fps" => {
                args.fps = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--fps takes a whole number (0: the display's rate)")?;
            }
            "--pam-service" => args.pam_service = it.next().ok_or("--pam-service takes a name")?,
            "-h" | "--help" => {
                print!("{}\n\n{USAGE}", config::TITLE);
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(args)
}

fn fail(why: impl std::fmt::Display) -> ! {
    eprintln!("{}: {why}", env!("CARGO_PKG_NAME"));
    std::process::exit(1);
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let user = current_user().unwrap_or_else(|| fail("cannot tell who is logged in"));

    let conn = Connection::connect_to_env().unwrap_or_else(|e| fail(e));
    let (globals, event_queue) = registry_queue_init(&conn).unwrap_or_else(|e| fail(e));
    let qh: QueueHandle<Locker> = event_queue.handle();
    let mut event_loop: EventLoop<Locker> = EventLoop::try_new().unwrap_or_else(|e| fail(e));

    let session_lock_state = SessionLockState::new(&globals, &qh);
    let lock = session_lock_state.lock(&qh).unwrap_or_else(|_| {
        fail(
            "the compositor does not offer ext-session-lock-v1 \
             (niri, sway and Hyprland do; KDE and GNOME do not)",
        )
    });

    // PAM answers come back through the event loop.
    let (checked, answers) = channel::channel::<(bool, Vec<String>)>();
    event_loop
        .handle()
        .insert_source(answers, |event, _, locker| {
            if let channel::Event::Msg((accepted, messages)) = event {
                locker.checked(accepted, messages);
            }
        })
        .unwrap_or_else(|e| fail(e));

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
        Box::new(Display(conn.clone())),
    ));
    let mut locker = Locker {
        conn: conn.clone(),
        registry_state: RegistryState::new(&globals),
        compositor_state: CompositorState::bind(&globals, &qh).unwrap_or_else(|e| fail(e)),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        _session_lock_state: session_lock_state,
        lock: Some(lock),
        outputs: Vec::new(),
        instance,
        gpu: None,
        fps: args.fps,
        start: Instant::now(),
        keyboard: None,
        modifiers: Modifiers::default(),
        password: Zeroizing::new(String::new()),
        checking: false,
        wrong_at: None,
        key_at: None,
        user,
        pam_service: args.pam_service,
        checked,
        messages: Vec::new(),
        messages_generation: 0,
        font: load_font(),
        exit: None,
    };

    // One lock surface per output; the compositor configures each, and
    // outputs plugged in later get theirs in `new_output`.
    for output in locker.output_state.outputs() {
        locker.add_output(&qh, output);
    }

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .unwrap_or_else(|e| fail(e));

    // With a cap, a timer paces the frames; without, the compositor's frame
    // callbacks do.
    if args.fps > 0 {
        let interval = Duration::from_secs_f64(1.0 / f64::from(args.fps));
        let tick_qh = qh.clone();
        event_loop
            .handle()
            .insert_source(Timer::from_duration(interval), move |_, _, locker| {
                for index in 0..locker.outputs.len() {
                    if locker.outputs[index].ready {
                        locker.draw(index, &tick_qh);
                    }
                }
                TimeoutAction::ToDuration(interval)
            })
            .unwrap_or_else(|e| fail(e));
    }

    let code = loop {
        if let Some(code) = locker.exit {
            break code;
        }
        if let Err(e) = event_loop.dispatch(None, &mut locker) {
            fail(e);
        }
    };
    // The GPU surfaces go before the Wayland surfaces they draw to.
    drop(locker);
    std::process::exit(code);
}

/// The login name of the user running this, from the password database.
fn current_user() -> Option<String> {
    // SAFETY: `getpwuid` returns null or a pointer to a static record that
    // stays valid until the next call; the name is copied out at once.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_name.is_null() {
            return None;
        }
        CStr::from_ptr((*entry).pw_name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}

/// Check `password` for `user` with PAM: the authentication and the account
/// checks (expired, locked) both have to pass. Also returns what PAM said
/// on the way.
fn authenticate(service: &str, user: &str, password: Zeroizing<String>) -> (bool, Vec<String>) {
    let answer = Answer {
        password,
        messages: Vec::new(),
    };
    let mut context = match pam_client::Context::new(service, Some(user), answer) {
        Ok(context) => context,
        Err(e) => {
            return (
                false,
                vec![format!("cannot start PAM service {service}: {e}")],
            );
        }
    };
    let accepted =
        context.authenticate(Flag::NONE).is_ok() && context.acct_mgmt(Flag::NONE).is_ok();
    (
        accepted,
        std::mem::take(&mut context.conversation_mut().messages),
    )
}

/// Answers PAM's password prompt with what was typed, and keeps its
/// messages.
struct Answer {
    password: Zeroizing<String>,
    messages: Vec<String>,
}

impl Answer {
    fn keep(&mut self, msg: &CStr) {
        let msg: String = msg
            .to_string_lossy()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let msg = msg.trim();
        if !msg.is_empty() && !self.messages.iter().any(|m| m == msg) {
            self.messages.push(msg.to_string());
        }
    }
}

impl ConversationHandler for Answer {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        Err(ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.password.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, msg: &CStr) {
        self.keep(msg);
    }

    fn error_msg(&mut self, msg: &CStr) {
        self.keep(msg);
    }
}

/// Where sans-serif fonts usually are, should `fc-match` be missing.
const FALLBACK_FONTS: &[&str] = &[
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
];

/// The system's sans-serif font, for PAM's messages; without one they are
/// not shown.
fn load_font() -> Option<FontVec> {
    let matched = std::process::Command::new("fc-match")
        .args(["--format=%{file}", "sans-serif"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    let font = matched
        .into_iter()
        .chain(FALLBACK_FONTS.iter().map(|path| path.to_string()))
        .filter_map(|path| std::fs::read(path).ok())
        .find_map(|bytes| FontVec::try_from_vec(bytes).ok());
    if font.is_none() {
        eprintln!("no font found; PAM's messages will not show");
    }
    font
}

/// `lines` as centered text, `px` pixels high, wrapped at `max_width`: its
/// width, height and one byte of coverage per pixel.
fn rasterize(font: &FontVec, lines: &[String], px: f32, max_width: f32) -> (u32, u32, Vec<u8>) {
    let font = font.as_scaled(PxScale::from(px));
    let width_of = |text: &str| {
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(previous) = previous {
                width += font.kern(previous, id);
            }
            width += font.h_advance(id);
            previous = Some(id);
        }
        width
    };
    let mut wrapped = Vec::new();
    for line in lines {
        let mut current = String::new();
        for word in line.split_whitespace() {
            let longer = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && width_of(&longer) > max_width {
                wrapped.push(std::mem::replace(&mut current, word.to_string()));
            } else {
                current = longer;
            }
        }
        if !current.is_empty() {
            wrapped.push(current);
        }
    }
    wrapped.truncate(8);
    let line_height = (font.ascent() - font.descent() + font.line_gap()).ceil();
    let widest = wrapped.iter().map(|l| width_of(l)).fold(0.0, f32::max);
    let width = widest.min(max_width).ceil() as u32 + 2;
    let height = (line_height * wrapped.len() as f32) as u32;
    let mut pixels = vec![0u8; (width * height) as usize];
    for (row, line) in wrapped.iter().enumerate() {
        let mut x = ((width as f32 - width_of(line)) / 2.0).max(0.0);
        let baseline = row as f32 * line_height + font.ascent();
        let mut previous = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(previous) = previous {
                x += font.kern(previous, id);
            }
            let glyph = id.with_scale_and_position(font.scale(), point(x, baseline));
            x += font.h_advance(id);
            previous = Some(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let x = bounds.min.x as i64 + i64::from(gx);
                let y = bounds.min.y as i64 + i64::from(gy);
                if (0..i64::from(width)).contains(&x) && (0..i64::from(height)).contains(&y) {
                    let pixel = &mut pixels[(y * i64::from(width) + x) as usize];
                    *pixel = (*pixel).max((coverage * 255.0).round() as u8);
                }
            });
        }
    }
    (width, height, pixels)
}

/// The Wayland display, for wgpu's GL backend (EGL needs it up front).
#[derive(Debug)]
struct Display(Connection);

impl HasDisplayHandle for Display {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let display =
            NonNull::new(self.0.backend().display_ptr().cast()).ok_or(HandleError::Unavailable)?;
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));
        // SAFETY: the connection, and so the display, outlives the borrow.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

struct Locker {
    conn: Connection,
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    seat_state: SeatState,
    _session_lock_state: SessionLockState,
    lock: Option<SessionLock>,
    /// Before `gpu`: the surfaces go before the device.
    outputs: Vec<Output>,
    instance: wgpu::Instance,
    /// Set up with the first output's surface, so the adapter can draw to it.
    gpu: Option<Gpu>,
    fps: u32,
    start: Instant,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    modifiers: Modifiers,
    password: Zeroizing<String>,
    /// PAM is checking the password; keys are ignored meanwhile.
    checking: bool,
    wrong_at: Option<Instant>,
    key_at: Option<Instant>,
    user: String,
    pam_service: String,
    checked: channel::Sender<(bool, Vec<String>)>,
    /// What PAM said about the last password, shown under the dots.
    messages: Vec<String>,
    /// Counts changes to `messages`, so outputs know to redraw the text.
    messages_generation: u64,
    font: Option<FontVec>,
    exit: Option<i32>,
}

/// One output's lock surface and what draws to it.
struct Output {
    /// Before `lock_surface`: it draws to that surface, so it goes first.
    target: Option<Target>,
    lock_surface: SessionLockSurface,
    output: wl_output::WlOutput,
    /// In logical pixels, as configured.
    size: (u32, u32),
    scale: i32,
    frame: u32,
    drawn_at: Option<Instant>,
    /// The compositor wants another frame (its frame callback came).
    ready: bool,
}

struct Target {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    uniforms: wgpu::Buffer,
    shader_group: wgpu::BindGroup,
    indicator: wgpu::Buffer,
    indicator_group: wgpu::BindGroup,
    /// PAM's messages, rasterized: size in pixels (0 for none), and the
    /// messages generation and scale they were drawn for.
    text_size: (u32, u32),
    text_for: Option<(u64, i32)>,
}

struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader_layout: wgpu::BindGroupLayout,
    indicator_layout: wgpu::BindGroupLayout,
    vertex: wgpu::ShaderModule,
    shader: wgpu::ShaderModule,
    indicator: wgpu::ShaderModule,
    /// The shader failed to build on this GPU: draw the plain background.
    shader_failed: bool,
    /// The shader and indicator pipelines, per surface format.
    pipelines: Vec<(
        wgpu::TextureFormat,
        wgpu::RenderPipeline,
        wgpu::RenderPipeline,
    )>,
}

impl Gpu {
    fn new(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>) -> Result<Self, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| e.to_string())?;
        let module = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        // The indicator reads PAM's messages from a texture, texel by texel.
        let text = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // A shader the driver rejects leaves the lock screen plain instead
        // of crashing it (which would leave the session locked and blank).
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = module("shader", include_str!("shader.wgsl"));
        let shader_failed = pollster::block_on(scope.pop()).is_some();
        Ok(Self {
            vertex: module("vertex", include_str!("vertex.wgsl")),
            indicator: module("indicator", include_str!("indicator.wgsl")),
            shader,
            shader_failed,
            shader_layout: layout(&[uniform(2)]),
            indicator_layout: layout(&[uniform(0), text]),
            adapter,
            device,
            queue,
            pipelines: Vec::new(),
        })
    }

    /// Build the shader and indicator pipelines for `format`, unless built.
    fn build_pipelines(&mut self, format: wgpu::TextureFormat) {
        if !self.pipelines.iter().any(|(f, ..)| *f == format) {
            let pipeline = |layout: &wgpu::BindGroupLayout,
                            module: &wgpu::ShaderModule,
                            entry: &str,
                            blend: Option<wgpu::BlendState>| {
                let layout = self
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[Some(layout)],
                        immediate_size: 0,
                    });
                self.device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(entry),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &self.vertex,
                            entry_point: Some("vertex"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    })
            };
            let mut background = None;
            if !self.shader_failed {
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                let built = pipeline(&self.shader_layout, &self.shader, "fragment", None);
                match pollster::block_on(scope.pop()) {
                    None => background = Some(built),
                    Some(e) => {
                        eprintln!("the shader does not build on this GPU: {e}");
                        self.shader_failed = true;
                    }
                }
            }
            let background = background.unwrap_or_else(|| {
                pipeline(&self.indicator_layout, &self.indicator, "plain", None)
            });
            let indicator = pipeline(
                &self.indicator_layout,
                &self.indicator,
                "indicator",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            );
            self.pipelines.push((format, background, indicator));
        }
    }

    /// The shader and indicator pipelines for `format`, once built.
    fn pipelines(
        &self,
        format: wgpu::TextureFormat,
    ) -> (&wgpu::RenderPipeline, &wgpu::RenderPipeline) {
        let (_, background, indicator) = self
            .pipelines
            .iter()
            .find(|(f, ..)| *f == format)
            .expect("built before drawing");
        (background, indicator)
    }

    /// A texture of `width` × `height` coverage bytes, and the indicator's
    /// bind group with it.
    fn indicator_group(
        &self,
        indicator: &wgpu::Buffer,
        (width, height): (u32, u32),
        pixels: &[u8],
    ) -> wgpu::BindGroup {
        use wgpu::util::DeviceExt;
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("messages"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        );
        let view = texture.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.indicator_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: indicator.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        })
    }
}

impl Locker {
    fn add_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(lock) = &self.lock else { return };
        let surface = self.compositor_state.create_surface(qh);
        let lock_surface = lock.create_lock_surface(surface, &output, qh);
        self.outputs.push(Output {
            target: None,
            lock_surface,
            output,
            size: (0, 0),
            scale: 1,
            frame: 0,
            drawn_at: None,
            ready: false,
        });
    }

    fn output_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|o| o.lock_surface.wl_surface() == surface)
    }

    /// Make the GPU surface for output `index` (the GPU too, the first
    /// time) and size it to the configured size and scale.
    fn configure_target(&mut self, index: usize) -> Result<(), String> {
        let out = &mut self.outputs[index];
        let width = out.size.0 * out.scale.max(1) as u32;
        let height = out.size.1 * out.scale.max(1) as u32;
        if width == 0 || height == 0 {
            return Ok(());
        }
        if let Some(target) = &mut out.target {
            let gpu = self.gpu.as_ref().expect("made with the target");
            target.config.width = width;
            target.config.height = height;
            target.surface.configure(&gpu.device, &target.config);
            return Ok(());
        }

        let display = NonNull::new(self.conn.backend().display_ptr().cast()).ok_or("no display")?;
        let window =
            NonNull::new(out.lock_surface.wl_surface().id().as_ptr().cast()).ok_or("no surface")?;
        // SAFETY: the Wayland surface outlives the GPU surface (`Output`
        // drops `target` first) and the connection outlives both.
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                        display,
                    ))),
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                })
                .map_err(|e| e.to_string())?
        };
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(&self.instance, &surface)?);
        }
        let gpu = self.gpu.as_ref().expect("made above");
        let caps = surface.get_capabilities(&gpu.adapter);
        let mut config = surface
            .get_default_config(&gpu.adapter, width, height)
            .ok_or("the GPU cannot draw to this output")?;
        // sRGB like the editor's canvas, so colors match what was exported.
        if let Some(&format) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = format;
        }
        if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            config.present_mode = wgpu::PresentMode::Mailbox;
        }
        if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        surface.configure(&gpu.device, &config);

        let buffer = |size: usize| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniforms = buffer(UNIFORM_WORDS * 4);
        let indicator = buffer(INDICATOR_WORDS * 4);
        out.target = Some(Target {
            shader_group: gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &gpu.shader_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                }],
            }),
            // No text until `draw` rasterizes some.
            indicator_group: gpu.indicator_group(&indicator, (1, 1), &[0]),
            surface,
            config,
            uniforms,
            indicator,
            text_size: (0, 0),
            text_for: None,
        });
        Ok(())
    }

    /// Draw a frame on output `index` and ask for the next one.
    fn draw(&mut self, index: usize, qh: &QueueHandle<Self>) {
        let now = Instant::now();
        let indicator = self.indicator_words(now);
        let time = now.duration_since(self.start).as_secs_f32();
        let fps = self.fps;
        let (Some(gpu), Some(out)) = (self.gpu.as_mut(), self.outputs.get_mut(index)) else {
            return;
        };
        let Some(target) = &mut out.target else {
            return;
        };
        out.ready = false;
        let wl_surface = out.lock_surface.wl_surface();
        // Ask before presenting: presenting commits the surface.
        wl_surface.frame(qh, wl_surface.clone());

        let texture = match target.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                target.surface.configure(&gpu.device, &target.config);
                wl_surface.commit();
                return;
            }
            _ => {
                wl_surface.commit();
                return;
            }
        };

        let dt = out
            .drawn_at
            .map_or(0.0, |at| now.duration_since(at).as_secs_f32());
        out.drawn_at = Some(now);
        out.frame = out.frame.wrapping_add(1);
        let (width, height) = (target.config.width as f32, target.config.height as f32);
        let frame_rate = if dt > 0.0 { 1.0 / dt } else { fps as f32 };
        let words = shader_words(time, dt, out.frame, (width, height), frame_rate);
        let scale = out.scale.max(1);
        if target.text_for != Some((self.messages_generation, scale)) {
            target.text_for = Some((self.messages_generation, scale));
            let text = self
                .font
                .as_ref()
                .filter(|_| !self.messages.is_empty())
                .map(|font| rasterize(font, &self.messages, 18.0 * scale as f32, width * 0.8))
                .filter(|&(w, h, _)| w > 0 && h > 0 && w <= 8192 && h <= 8192);
            let (size, pixels) = match &text {
                Some((w, h, pixels)) => ((*w, *h), pixels.as_slice()),
                None => ((1, 1), &[0][..]),
            };
            target.indicator_group = gpu.indicator_group(&target.indicator, size, pixels);
            target.text_size = if text.is_some() { size } else { (0, 0) };
        }
        let mut indicator = indicator;
        indicator[..3].copy_from_slice(&[width, height, scale as f32]);
        indicator[8] = target.text_size.0 as f32;
        indicator[9] = target.text_size.1 as f32;
        gpu.queue.write_buffer(&target.uniforms, 0, &bytes(&words));
        gpu.queue
            .write_buffer(&target.indicator, 0, &bytes(&indicator.map(f32::to_bits)));

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        gpu.build_pipelines(target.config.format);
        let (background, overlay) = gpu.pipelines(target.config.format);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
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
            pass.set_pipeline(background);
            if gpu.shader_failed {
                pass.set_bind_group(0, &target.indicator_group, &[]);
            } else {
                pass.set_bind_group(0, &target.shader_group, &[]);
            }
            pass.draw(0..3, 0..1);
            if indicator[3] > 0.0 {
                pass.set_pipeline(overlay);
                pass.set_bind_group(0, &target.indicator_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(texture);
    }

    /// The indicator's uniforms but for the sizes (filled in per output):
    /// size, scale, opacity; typed characters, state (0 typing,
    /// 1 checking, 2 wrong), caps lock, seconds since locking; the text's
    /// size.
    fn indicator_words(&self, now: Instant) -> [f32; INDICATOR_WORDS] {
        let wrong = self
            .wrong_at
            .is_some_and(|at| now.duration_since(at) < WRONG_FOR);
        let show_for = if self.messages.is_empty() {
            SHOW_FOR
        } else {
            SHOW_MESSAGES_FOR
        };
        let recent = self
            .key_at
            .map(|at| now.duration_since(at))
            .filter(|&since| since < show_for);
        let opacity = if self.checking || wrong || !self.password.is_empty() {
            1.0
        } else {
            // Fade out over the last half second.
            recent.map_or(0.0, |since| {
                ((show_for - since).as_secs_f32() / 0.5).min(1.0)
            })
        };
        let state = if self.checking {
            1.0
        } else if wrong {
            2.0
        } else {
            0.0
        };
        [
            0.0,
            0.0,
            1.0,
            opacity,
            self.password.chars().count() as f32,
            state,
            if self.modifiers.caps_lock { 1.0 } else { 0.0 },
            now.duration_since(self.start).as_secs_f32(),
            0.0,
            0.0,
            0.0,
            0.0,
        ]
    }

    fn key(&mut self, event: KeyEvent) {
        self.key_at = Some(Instant::now());
        if self.checking {
            return;
        }
        match event.keysym {
            Keysym::Return | Keysym::KP_Enter => {
                if self.password.is_empty() {
                    return;
                }
                self.checking = true;
                let password = std::mem::replace(&mut self.password, Zeroizing::new(String::new()));
                let (service, user) = (self.pam_service.clone(), self.user.clone());
                let checked = self.checked.clone();
                std::thread::spawn(move || {
                    let _ = checked.send(authenticate(&service, &user, password));
                });
            }
            Keysym::BackSpace => {
                self.password.pop();
            }
            Keysym::Escape => self.password.clear(),
            Keysym::u if self.modifiers.ctrl => self.password.clear(),
            _ if self.modifiers.ctrl || self.modifiers.alt || self.modifiers.logo => {}
            _ => {
                if let Some(text) = &event.utf8 {
                    self.password
                        .extend(text.chars().filter(|c| !c.is_control()));
                }
            }
        }
    }

    fn checked(&mut self, accepted: bool, messages: Vec<String>) {
        self.checking = false;
        // Count the answer as activity, so its messages stay up to be read.
        self.key_at = Some(Instant::now());
        if messages != self.messages {
            self.messages = messages;
            self.messages_generation += 1;
        }
        if accepted {
            if let Some(lock) = self.lock.take() {
                lock.unlock();
            }
            // Make sure the compositor got the unlock before exiting.
            let _ = self.conn.roundtrip();
            self.exit = Some(0);
        } else {
            self.wrong_at = Some(Instant::now());
        }
    }
}

/// The shader's uniforms, laid out like `ShaderToyUniforms`.
fn shader_words(
    time: f32,
    dt: f32,
    frame: u32,
    (width, height): (f32, f32),
    frame_rate: f32,
) -> [u32; UNIFORM_WORDS] {
    let mut w = [0u32; UNIFORM_WORDS];
    w[0] = time.to_bits();
    w[1] = dt.to_bits();
    w[2] = frame;
    w[4] = width.to_bits();
    w[5] = height.to_bits();
    w[6] = 1.0f32.to_bits();
    // 8..12: iMouse stays zero; there is no pointer on a lock screen.
    for (i, v) in date().into_iter().enumerate() {
        w[12 + i] = v.to_bits();
    }
    w[16] = 44100.0f32.to_bits();
    w[17] = frame_rate.to_bits();
    // 18..40: the editor's overlay insets and the buffer channels, unused.
    for (slot, value) in config::TWEAKS.iter().enumerate() {
        for (i, v) in value.iter().enumerate() {
            w[40 + 4 * slot + i] = v.to_bits();
        }
    }
    w
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// shadertoy's `iDate`, in local time: year, month (0 = January), day of
/// month, seconds since midnight.
fn date() -> [f32; 4] {
    let now = jiff::Zoned::now();
    let seconds = f64::from(now.hour()) * 3600.0
        + f64::from(now.minute()) * 60.0
        + f64::from(now.second())
        + f64::from(now.subsec_nanosecond()) * 1e-9;
    [
        f32::from(now.year()),
        f32::from(now.month() - 1),
        f32::from(now.day()),
        seconds as f32,
    ]
}

impl SessionLockHandler for Locker {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {}

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        // The compositor refused (another locker holds the session) or
        // ended the lock.
        if self.lock.take().is_some() {
            eprintln!(
                "{}: the session could not be locked",
                env!("CARGO_PKG_NAME")
            );
        }
        self.exit = Some(1);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self.output_of(surface.wl_surface()) else {
            return;
        };
        self.outputs[index].size = configure.new_size;
        if let Err(e) = self.configure_target(index) {
            // Without a buffer the output stays as the compositor shows a
            // lock surface it has nothing for: locked and blank.
            eprintln!("cannot draw on an output: {e}");
            return;
        }
        self.draw(index, qh);
    }
}

impl CompositorHandler for Locker {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        let Some(index) = self.output_of(surface) else {
            return;
        };
        if self.outputs[index].scale != factor {
            self.outputs[index].scale = factor;
            surface.set_buffer_scale(factor);
            if let Err(e) = self.configure_target(index) {
                eprintln!("cannot draw on an output: {e}");
            }
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        let Some(index) = self.output_of(surface) else {
            return;
        };
        if self.fps == 0 {
            self.draw(index, qh);
        } else {
            self.outputs[index].ready = true;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Locker {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if !self.outputs.iter().any(|o| o.output == output) {
            self.add_output(qh, output);
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.outputs.retain(|o| o.output != output);
    }
}

impl SeatHandler for Locker {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(e) => eprintln!("no keyboard: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Locker {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        // Holding Backspace clears; holding Enter must not resubmit.
        if event.keysym == Keysym::BackSpace {
            self.key(event);
        }
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl ProvidesRegistryState for Locker {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Locker);
delegate_output!(Locker);
delegate_seat!(Locker);
delegate_keyboard!(Locker);
delegate_session_lock!(Locker);
delegate_registry!(Locker);
