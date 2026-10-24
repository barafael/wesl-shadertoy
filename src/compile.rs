//! Runtime WESL compilation for wesl-shadertoy.
//!
//! The editor text is compiled *verbatim* as the virtual WESL module
//! `package::user`, so diagnostics from the WESL compiler carry the exact line
//! numbers the user sees in the editor. Two hidden modules are added:
//!
//! * `package::shadertoy` — declares the uniform block (bound at
//!   `@group(0) @binding(2)`, the slot the fullscreen-material pipeline uses
//!   for its dynamic uniform buffer), the four buffer channel textures (bound
//!   at `@group(0) @binding(3..6)`) and shadertoy-style accessor functions
//!   (`i_time()`, `i_resolution()`, ...) plus the importable `i_channel_0`..
//!   `i_channel_3` / `i_sampler` handle globals.
//! * `package::main` — the entry module, generated per compile. It imports
//!   `user::main_image` and defines the `@fragment fn fragment` entry point
//!   that the pipeline looks for, flipping `@builtin(position)` to shadertoy's
//!   bottom-left `fragCoord` origin on the way in. For every `buffer_a`..
//!   `buffer_d` function the user defines it also emits a
//!   `fragment_buffer_a`..`fragment_buffer_d` entry point, which the renderer
//!   draws into that channel's offscreen target before the main pass; those
//!   entries pass `@builtin(position)` through unflipped (texel space), so
//!   `textureLoad` reads line up with the writes (see [`main_module_for`]).
//!
//! The composed WGSL is then parsed and validated with `naga` (same major
//! version as the wgpu inside Bevy 0.20) so type errors in user code are
//! reported in the UI instead of only in the browser console.

use std::str::FromStr;

use wesl::mangler::{EscapeMangler, Mangler};
use wesl::resolver::VirtualResolver;
use wesl::syntax::ModulePath;
use wesl::{CompileOptions, Compiler};

/// The WGSL uniform block, shared by [`SHADERTOY_MODULE`] and the debugger
/// overlay ([`crate::debugger::DEBUGGER_WGSL`]) so the two cannot drift.
/// Field order must match [`crate::render::ShaderToyUniform`]; the
/// `uniform_layout_matches_rust` test checks every member's offset.
macro_rules! shadertoy_uniforms_wgsl {
    () => {
        r#"struct ShaderToyUniforms {
    i_time: f32,
    i_time_delta: f32,
    i_frame: u32,
    _pad: f32,
    i_resolution: vec4<f32>,
    i_mouse: vec4<f32>,
    i_date: vec4<f32>,
    i_sample_rate: f32,
    i_frame_rate: f32,
    // Debugger overlay only (canvas rows hidden under the toolbar / status
    // bar, physical px); not part of the shader API.
    _overlay_top: f32,
    _overlay_bottom: f32,
    i_channel_time: vec4<f32>,
    i_channel_resolution: array<vec4<f32>, 4>,
}
"#
    };
}
pub(crate) use shadertoy_uniforms_wgsl;

/// The hidden `package::shadertoy` module: uniforms + accessors.
pub(crate) const SHADERTOY_MODULE: &str = concat!(
    r#"// The shadertoy environment, provided by wesl-shadertoy.
// Uniforms live in the dynamic uniform buffer the fullscreen-material
// pipeline binds at @group(0) @binding(2).

struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

"#,
    shadertoy_uniforms_wgsl!(),
    r#"
@group(0) @binding(2) var<uniform> st_uniforms: ShaderToyUniforms;
/// The shared nearest, clamp-to-edge sampler. Buffer channels are
/// `Rgba32Float` (like shadertoy's buffers) and therefore unfilterable, so
/// sampling them through `i_sampler` is nearest-neighbour; for smooth reads
/// use `textureLoad` with hand-rolled bilinear filtering.
@group(0) @binding(1) var i_sampler: sampler;
/// Buffer channel A: the `buffer_a` pass result, or a black 1x1 texture when
/// the shader defines no `buffer_a`. Import it directly — WGSL handle types
/// cannot be returned from functions. Read texels with
/// `textureLoad(i_channel_0, coords, 0)`, or sample nearest with
/// `textureSampleLevel(i_channel_0, i_sampler, uv, 0.0)`.
@group(0) @binding(3) var i_channel_0: texture_2d<f32>;
/// Buffer channel B (`buffer_b`).
@group(0) @binding(4) var i_channel_1: texture_2d<f32>;
/// Buffer channel C (`buffer_c`).
@group(0) @binding(5) var i_channel_2: texture_2d<f32>;
/// Buffer channel D (`buffer_d`).
@group(0) @binding(6) var i_channel_3: texture_2d<f32>;

/// Seconds since (re)start.
fn i_time() -> f32 { return st_uniforms.i_time; }
/// Seconds since the previous frame.
fn i_time_delta() -> f32 { return st_uniforms.i_time_delta; }
/// Frame counter.
fn i_frame() -> u32 { return st_uniforms.i_frame; }
/// Viewport size in pixels (z = 1.0).
fn i_resolution() -> vec3<f32> { return st_uniforms.i_resolution.xyz; }
/// xy: pointer position while dragging (else last position),
/// zw: click position; positive while a button is held, negative after
/// release. Origin bottom-left, like shadertoy's iMouse.
fn i_mouse() -> vec4<f32> { return st_uniforms.i_mouse; }
/// x: year, y: month (1-12), z: day of month, w: seconds since midnight.
fn i_date() -> vec4<f32> { return st_uniforms.i_date; }
/// Audio sample rate in Hz. Fixed at 44100 — there is no audio input.
fn i_sample_rate() -> f32 { return st_uniforms.i_sample_rate; }
/// Smoothed frames per second.
fn i_frame_rate() -> f32 { return st_uniforms.i_frame_rate; }
/// Playback time of buffer channel i (0..3); 0.0 while the channel is unused.
fn i_channel_time(i: u32) -> f32 { return st_uniforms.i_channel_time[i]; }
/// Size of buffer channel i (0..3) in pixels (z = 0 for 2D buffers);
/// (0, 0, 0) while the channel is unused.
fn i_channel_resolution(i: u32) -> vec3<f32> { return st_uniforms.i_channel_resolution[i].xyz; }
"#
);

/// Names of the optional per-frame buffer passes, in shadertoy's A→D
/// evaluation order. Bit *i* of a buffer mask means `BUFFER_NAMES[i]` is
/// defined by the user shader.
pub const BUFFER_NAMES: [&str; 4] = ["buffer_a", "buffer_b", "buffer_c", "buffer_d"];

/// The fragment entry point of the main image pass. The fullscreen material
/// pipeline selects it by name, since the module may also carry
/// `fragment_buffer_*` siblings.
pub const MAIN_ENTRY: &str = "fragment";

/// The fragment entry point that renders buffer pass `name` (`buffer_a` ..
/// `buffer_d`); shared by [`main_module_for`] and the buffer renderer.
pub fn buffer_entry(name: &str) -> String {
    format!("fragment_{name}")
}

/// The fixed part of the hidden entry module `package::main`; buffer entry
/// points are appended by [`main_module_for`].
const MAIN_MODULE_HEAD: &str = r#"import package::shadertoy::FullscreenVertexOutput;
import package::shadertoy::i_resolution;
import package::user::main_image;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // shadertoy's fragCoord has its origin bottom-left; wgpu's
    // @builtin(position) is top-left, so flip y to match shadertoy.
    let p = in.position.xy;
    return main_image(vec2<f32>(p.x, i_resolution().y - p.y));
}
"#;

/// Build the `package::main` module for a shader that defines the buffer
/// passes selected by `buffers` (bit per [`BUFFER_NAMES`] entry). Each buffer
/// gets its own fragment entry point, rendered into that channel's offscreen
/// target; undefined buffers get a stub entry (see the comment inside).
///
/// Buffer entries pass `@builtin(position)` through unflipped, so a buffer's
/// `frag_coord.y` equals its texture row. A fragment always writes the texel
/// at its own position, so this is what keeps
/// `textureLoad(i_channel_x, vec2<i32>(frag_coord), 0)` consistent with where
/// the content was written — flipping on write would mirror every channel
/// read. The y-up image pass samples channels at its own `frag_coord`, which
/// displays the buffer space bottom-up just like the image space (and like
/// shadertoy's `texture(iChannel0, fragCoord / resolution)`): `frag_coord.y`
/// grows upward on screen and `i_mouse()` lines up with the cursor.
fn main_module_for(buffers: u8) -> String {
    let mut wgsl = String::new();
    // Imports must come first
    for (i, name) in BUFFER_NAMES.iter().enumerate() {
        if buffers & (1 << i) != 0 {
            wgsl.push_str(&format!("import package::user::{name};\n"));
        }
    }
    wgsl.push_str(MAIN_MODULE_HEAD);
    for (i, name) in BUFFER_NAMES.iter().enumerate() {
        let body = if buffers & (1 << i) != 0 {
            format!(
                "\x20   // Buffer passes use texel coordinates (no y-flip).\n\
                 \x20   return {name}(in.position.xy);\n"
            )
        } else {
            // Never rendered, but the renderer reuses shader slots: a
            // pipeline queued for this entry under an earlier shader is
            // rebuilt against this one, and must still find its entry point.
            "\x20   // No buffer pass: stub entry, never rendered.\n\
             \x20   return vec4<f32>(0.0);\n"
                .to_string()
        };
        wgsl.push_str(&format!(
            "\n@fragment\n\
             fn {}(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {{\n{body}}}\n",
            buffer_entry(name)
        ));
    }
    wgsl
}

/// The result of compiling the editor text.
#[derive(Debug)]
pub struct CompiledShader {
    /// Complete WGSL module ready for `Shader::from_wgsl`.
    pub wgsl: String,
    /// Bit per [`BUFFER_NAMES`] entry the shader defines.
    pub buffers: u8,
}

#[derive(Debug)]
pub enum CompileError {
    /// WESL front-end (parse / import resolution / conditional compilation).
    Wesl(String),
    /// The composed WGSL failed to parse.
    WgslParse(String),
    /// The composed WGSL failed naga validation (type errors etc.).
    WgslValidate(String),
    /// The shader binds a resource the pipeline does not provide.
    Binding(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Wesl(s) => write!(f, "WESL error:\n{s}"),
            CompileError::WgslParse(s) => write!(f, "WGSL parse error:\n{s}"),
            CompileError::WgslValidate(s) => write!(f, "Shader validation error:\n{s}"),
            CompileError::Binding(s) => write!(f, "Binding error:\n{s}"),
        }
    }
}

/// Compile the editor text into a complete WGSL module ready for
/// `Shader::from_wgsl`, detecting which `buffer_a`..`buffer_d` passes it
/// defines so the entry module (and renderer) can grow to match.
pub fn compile_user_shader(user_source: &str) -> Result<CompiledShader, CompileError> {
    let buffers = detect_buffers(user_source)?;

    let mut resolver = VirtualResolver::new();
    resolver.add_module(
        ModulePath::from_str("package::main").expect("valid module path"),
        main_module_for(buffers).into(),
    );
    resolver.add_module(
        ModulePath::from_str("package::shadertoy").expect("valid module path"),
        SHADERTOY_MODULE.into(),
    );
    resolver.add_module(
        ModulePath::from_str("package::user").expect("valid module path"),
        user_source.to_string().into(),
    );

    let compiler = Compiler::new_with_resolver(CompileOptions::default(), resolver);
    let main_path = ModulePath::from_str("package::main").expect("valid module path");
    let result = compiler
        .compile_module(&main_path)
        .map_err(|e| CompileError::Wesl(e.to_string()))?;
    let wgsl = result.to_string();

    // Pre-validate with the same naga family wgpu uses, so user type errors
    // surface in the UI instead of as a silently stale pipeline.
    let module = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|e| CompileError::WgslParse(e.emit_to_string(&wgsl)))?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    validator
        .validate(&module)
        .map_err(|e| CompileError::WgslValidate(e.emit_to_string(&wgsl)))?;
    check_bindings(&module)?;

    Ok(CompiledShader { wgsl, buffers })
}

/// Reject resource bindings that `package::shadertoy` does not declare.
///
/// The pipeline layout provides exactly the hidden module's resources. naga
/// accepts any binding, but wgpu rejects a pipeline whose shader uses a slot
/// the layout lacks or types differently, and Bevy's default render error
/// handler quits the app on that error, taking unsaved edits with it.
fn check_bindings(module: &naga::Module) -> Result<(), CompileError> {
    let env = ModulePath::from_str("package::shadertoy").expect("valid module path");
    let foreign: Vec<String> = module
        .global_variables
        .iter()
        .filter_map(|(_, var)| {
            let binding = var.binding.as_ref()?;
            let mangled = var.name.as_deref().unwrap_or("?");
            let (from_env, item) = match EscapeMangler.unmangle(mangled) {
                Some((path, item)) => (path == env, item),
                None => (false, mangled.to_string()),
            };
            (!from_env).then(|| {
                format!(
                    "`{item}` at @group({}) @binding({})",
                    binding.group, binding.binding
                )
            })
        })
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    Err(CompileError::Binding(format!(
        "{} binds resources wesl-shadertoy does not provide. Import them from \
         package::shadertoy instead (i_sampler, i_channel_0..i_channel_3; the \
         uniforms through i_time() and friends).",
        foreign.join(", ")
    )))
}

/// Compile `package::user` on its own and report which buffer passes it
/// defines, by looking for `fn buffer_a`..`buffer_d` in the output.
fn detect_buffers(user_source: &str) -> Result<u8, CompileError> {
    let mut resolver = VirtualResolver::new();
    resolver.add_module(
        ModulePath::from_str("package::shadertoy").expect("valid module path"),
        SHADERTOY_MODULE.into(),
    );
    resolver.add_module(
        ModulePath::from_str("package::user").expect("valid module path"),
        user_source.to_string().into(),
    );

    // `keep_main` disables stripping *within* package::user, so unreferenced
    // `buffer_*` functions survive to be detected (the real compile in
    // `compile_user_shader` keeps stripping on: `main_image` is reachable
    // from the generated `package::main` entry points there).
    let options = CompileOptions {
        keep_main: true,
        ..Default::default()
    };
    let compiler = Compiler::new_with_resolver(options, resolver);
    let user_path = ModulePath::from_str("package::user").expect("valid module path");
    let result = compiler
        .compile_module(&user_path)
        .map_err(|e| CompileError::Wesl(e.to_string()))?;
    let wgsl = result.to_string();
    let module = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|e| CompileError::WgslParse(e.emit_to_string(&wgsl)))?;

    let mut buffers = 0u8;
    for (i, name) in BUFFER_NAMES.iter().enumerate() {
        if module
            .functions
            .iter()
            .any(|(_, f)| f.name.as_deref() == Some(*name))
        {
            buffers |= 1 << i;
        }
    }
    Ok(buffers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_minimal_shader() {
        let src = r#"
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();
    let col = 0.5 + 0.5 * cos(t + uv.xyx + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;
        let wgsl = compile_user_shader(src).expect("should compile").wgsl;
        // The renderer selects this entry point by name, so pin the exact
        // declaration (a bare `contains("fragment")` would also match the
        // buffer siblings and let the names drift apart).
        assert!(
            wgsl.contains(&format!("fn {MAIN_ENTRY}(")),
            "entry point missing:\n{wgsl}"
        );
        assert!(wgsl.contains("@group(0) @binding(2)"));
        // frag_coord must arrive with shadertoy's bottom-left origin.
        assert!(
            wgsl.contains("i_resolution().y - p.y"),
            "no y-flip:\n{wgsl}"
        );
    }

    #[test]
    fn compiles_channel_and_clock_accessors() {
        let src = r#"
import package::shadertoy::i_sample_rate;
import package::shadertoy::i_frame_rate;
import package::shadertoy::i_channel_time;
import package::shadertoy::i_channel_resolution;
import package::shadertoy::i_date;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var acc = i_sample_rate() + i_frame_rate() + i_date().w * 1e-5;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        acc = acc + i_channel_time(i) + i_channel_resolution(i).x;
    }
    return vec4<f32>(vec3<f32>(acc), 1.0);
}
"#;
        compile_user_shader(src).expect("should compile");
    }

    /// The Rust uniform struct and the WGSL `ShaderToyUniforms` struct are
    /// maintained on both sides of FFI-bytes: every WGSL member must sit at
    /// the offset where the Rust field of the same name is encoded, and the
    /// sizes must agree. (The debugger overlay shares the WGSL struct through
    /// `shadertoy_uniforms_wgsl!`, so this covers it too.)
    #[test]
    fn uniform_layout_matches_rust() {
        use bevy::math::Vec4;
        use bevy::render::render_resource::ShaderType;

        use crate::render::ShaderToyUniform;

        // Every 4-byte slot gets a distinct value, so the encoded bytes tell
        // which Rust field landed where.
        let v = |a: f32| Vec4::new(a, a + 1.0, a + 2.0, a + 3.0);
        let rust = ShaderToyUniform {
            time: 1.0,
            time_delta: 2.0,
            frame: 3,
            _pad: 4.0,
            resolution: v(5.0),
            mouse: v(9.0),
            date: v(13.0),
            sample_rate: 17.0,
            frame_rate: 18.0,
            overlay_top: 19.0,
            overlay_bottom: 20.0,
            channel_time: v(21.0),
            channel_resolution: [v(25.0), v(29.0), v(33.0), v(37.0)],
        };
        let bytes = crate::render::uniform_bytes(&rust);
        assert_eq!(bytes.len() as u64, ShaderToyUniform::min_size().get());
        let word = |offset: u32| {
            let o = offset as usize;
            u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap())
        };
        // WGSL member name -> the first word its Rust namesake encodes.
        let expected = [
            ("i_time", 1.0f32.to_bits()),
            ("i_time_delta", 2.0f32.to_bits()),
            ("i_frame", 3),
            ("_pad", 4.0f32.to_bits()),
            ("i_resolution", 5.0f32.to_bits()),
            ("i_mouse", 9.0f32.to_bits()),
            ("i_date", 13.0f32.to_bits()),
            ("i_sample_rate", 17.0f32.to_bits()),
            ("i_frame_rate", 18.0f32.to_bits()),
            ("_overlay_top", 19.0f32.to_bits()),
            ("_overlay_bottom", 20.0f32.to_bits()),
            ("i_channel_time", 21.0f32.to_bits()),
            ("i_channel_resolution", 25.0f32.to_bits()),
        ];

        let module =
            naga::front::wgsl::parse_str(SHADERTOY_MODULE).expect("shadertoy module parses");
        let (handle, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ShaderToyUniforms"))
            .expect("ShaderToyUniforms present");
        let naga::TypeInner::Struct { members, .. } = &ty.inner else {
            panic!("ShaderToyUniforms is a struct");
        };
        let names: Vec<_> = members.iter().filter_map(|m| m.name.as_deref()).collect();
        let expected_names: Vec<_> = expected.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, expected_names);
        for (member, (name, first_word)) in members.iter().zip(expected) {
            assert_eq!(
                word(member.offset),
                first_word,
                "{name} at offset {}",
                member.offset
            );
        }
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        assert_eq!(u64::from(layouter[handle].size), bytes.len() as u64);
    }

    /// Resources the pipeline does not provide are a compile error, not a
    /// pipeline-creation failure (which quits the app).
    #[test]
    fn rejects_foreign_bindings() {
        let src = r#"
import package::shadertoy::{i_channel_0, i_sampler};
@group(0) @binding(7) var extra: texture_2d<f32>;
@group(1) @binding(0) var<uniform> mine: vec4<f32>;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let uv = frag_coord * 0.001;
    return textureSampleLevel(i_channel_0, i_sampler, uv, 0.0)
        + textureLoad(extra, vec2<i32>(0), 0) + mine;
}
"#;
        let msg = compile_user_shader(src).unwrap_err().to_string();
        assert!(msg.starts_with("Binding error"), "unexpected error: {msg}");
        assert!(msg.contains("`extra` at @group(0) @binding(7)"), "{msg}");
        assert!(msg.contains("`mine` at @group(1) @binding(0)"), "{msg}");
        assert!(!msg.contains("i_sampler`"), "environment flagged: {msg}");
    }

    #[test]
    fn reports_missing_entry_point() {
        let err = compile_user_shader("fn not_main_image() {} ").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("main_image"), "unexpected error: {msg}");
    }

    #[test]
    fn detects_buffer_entries() {
        let src = r#"
import package::shadertoy::i_channel_0;
import package::shadertoy::i_resolution;
import package::shadertoy::i_sampler;

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(0.5, 0.0, 0.0, 1.0);
}

fn buffer_c(frag_coord: vec2<f32>) -> vec4<f32> {
    return textureSample(i_channel_0, i_sampler, frag_coord);
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return textureSample(i_channel_0, i_sampler, frag_coord / i_resolution().xy);
}
"#;
        let compiled = compile_user_shader(src).expect("should compile");
        assert_eq!(compiled.buffers, 0b0101, "expected buffer_a and buffer_c");
        assert!(compiled.wgsl.contains("fn fragment_buffer_a"));
        assert!(compiled.wgsl.contains("fn fragment_buffer_c"));
        // Undefined buffers still get a (stub) entry, for slot reuse.
        let stub = compiled
            .wgsl
            .split("fn fragment_buffer_b")
            .nth(1)
            .expect("stub entry for buffer_b");
        assert!(
            stub.split("@fragment")
                .next()
                .unwrap()
                .contains("vec4<f32>(0.0)")
        );
        // Buffer entries feed raw texel coordinates (no y-flip), so channel
        // reads line up with where the pass wrote; only the image pass flips.
        let entry = compiled
            .wgsl
            .split("fn fragment_buffer_a")
            .nth(1)
            .expect("buffer entry")
            .split('}')
            .next()
            .unwrap();
        assert!(
            entry.contains("in.position.xy"),
            "no texel passthrough:\n{entry}"
        );
        assert!(
            !entry.contains("i_resolution"),
            "unexpected y-flip:\n{entry}"
        );
    }

    #[test]
    fn buffer_names_in_comments_are_not_entries() {
        let src = r#"
// buffer_a is just a comment, not a pass.
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(1.0);
}
"#;
        assert_eq!(compile_user_shader(src).expect("should compile").buffers, 0);
    }

    #[test]
    fn reports_type_errors() {
        let src = r#"
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let bad: vec3<f32> = frag_coord + vec3<f32>(1.0);
    return vec4<f32>(bad, 1.0);
}
"#;
        let err = compile_user_shader(src).unwrap_err();
        // Either wesl validation or naga should catch the arity mismatch.
        let _msg = err.to_string();
    }
}
