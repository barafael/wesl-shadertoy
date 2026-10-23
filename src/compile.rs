//! Runtime WESL compilation for wesl-shadertoy.
//!
//! The editor text is compiled *verbatim* as the virtual WESL module
//! `package::user`, so diagnostics from the WESL compiler carry the exact line
//! numbers the user sees in the editor. Two hidden modules are added:
//!
//! * `package::shadertoy` — declares the uniform block (bound at
//!   `@group(0) @binding(2)`, the slot the fullscreen-material pipeline uses
//!   for its dynamic uniform buffer) and shadertoy-style accessor functions
//!   (`i_time()`, `i_resolution()`, ...).
//! * `package::main` — the entry module. It imports `user::main_image` and
//!   defines the `@fragment fn fragment` entry point that the pipeline looks
//!   for, flipping `@builtin(position)` to shadertoy's bottom-left `fragCoord`
//!   origin on the way in.
//!
//! The composed WGSL is then parsed and validated with `naga` (same major
//! version as the wgpu inside Bevy 0.20) so type errors in user code are
//! reported in the UI instead of only in the browser console.

use std::str::FromStr;

use wesl::resolver::VirtualResolver;
use wesl::syntax::ModulePath;
use wesl::{CompileOptions, Compiler};

/// The hidden `package::shadertoy` module: uniforms + accessors.
pub(crate) const SHADERTOY_MODULE: &str = r#"// The shadertoy environment, provided by wesl-shadertoy.
// Uniforms live in the dynamic uniform buffer the fullscreen-material
// pipeline binds at @group(0) @binding(2).

struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct ShaderToyUniforms {
    i_time: f32,
    i_time_delta: f32,
    i_frame: u32,
    _pad: f32,
    i_resolution: vec4<f32>,
    i_mouse: vec4<f32>,
    i_date: vec4<f32>,
    i_sample_rate: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
    i_channel_time: vec4<f32>,
    i_channel_resolution: array<vec4<f32>, 4>,
}

@group(0) @binding(2) var<uniform> st_uniforms: ShaderToyUniforms;

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
/// Playback time of buffer channel i (0..3); 0.0 while the channel is unused.
fn i_channel_time(i: u32) -> f32 { return st_uniforms.i_channel_time[i]; }
/// Size of buffer channel i (0..3) in pixels (z = 0 for 2D buffers);
/// (0, 0, 0) while the channel is unused.
fn i_channel_resolution(i: u32) -> vec3<f32> { return st_uniforms.i_channel_resolution[i].xyz; }
"#;

/// The hidden entry module `package::main`.
pub(crate) const MAIN_MODULE: &str = r#"import package::shadertoy::FullscreenVertexOutput;
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

#[derive(Debug)]
pub enum CompileError {
    /// WESL front-end (parse / import resolution / conditional compilation).
    Wesl(String),
    /// The composed WGSL failed to parse.
    WgslParse(String),
    /// The composed WGSL failed naga validation (type errors etc.).
    WgslValidate(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Wesl(s) => write!(f, "WESL error:\n{s}"),
            CompileError::WgslParse(s) => write!(f, "WGSL parse error:\n{s}"),
            CompileError::WgslValidate(s) => write!(f, "Shader validation error:\n{s}"),
        }
    }
}

/// Compile the editor text into a complete WGSL module ready for
/// `Shader::from_wgsl`.
pub fn compile_user_shader(user_source: &str) -> Result<String, CompileError> {
    let mut resolver = VirtualResolver::new();
    resolver.add_module(
        ModulePath::from_str("package::main").expect("valid module path"),
        MAIN_MODULE.into(),
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

    Ok(wgsl)
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
        let wgsl = compile_user_shader(src).expect("should compile");
        assert!(wgsl.contains("fragment"), "entry point missing:\n{wgsl}");
        assert!(wgsl.contains("@group(0) @binding(2)"));
        // frag_coord must arrive with shadertoy's bottom-left origin.
        assert!(wgsl.contains("i_resolution().y - p.y"), "no y-flip:\n{wgsl}");
    }

    #[test]
    fn compiles_channel_and_clock_accessors() {
        let src = r#"
import package::shadertoy::i_sample_rate;
import package::shadertoy::i_channel_time;
import package::shadertoy::i_channel_resolution;
import package::shadertoy::i_date;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var acc = i_sample_rate() + i_date().w * 1e-5;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        acc = acc + i_channel_time(i) + i_channel_resolution(i).x;
    }
    return vec4<f32>(vec3<f32>(acc), 1.0);
}
"#;
        compile_user_shader(src).expect("should compile");
    }

    /// The Rust uniform struct and the WGSL `ShaderToyUniforms` struct are
    /// maintained on both sides of FFI-bytes; guard that their sizes agree.
    #[test]
    fn rust_uniform_struct_matches_wgsl_layout() {
        use bevy::render::render_resource::ShaderType;

        use crate::render::ShaderToyUniform;

        let module = naga::front::wgsl::parse_str(SHADERTOY_MODULE)
            .expect("shadertoy module parses");
        let handle = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ShaderToyUniforms"))
            .map(|(h, _)| h)
            .expect("ShaderToyUniforms present");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        let wgsl_size = u64::from(layouter[handle].size);
        assert_eq!(wgsl_size, ShaderToyUniform::min_size().get());
    }

    #[test]
    fn reports_missing_entry_point() {
        let err = compile_user_shader("fn not_main_image() {} ").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("main_image"), "unexpected error: {msg}");
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
