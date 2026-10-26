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
//! User code may also import the bundled [LYGIA](https://lygia.xyz) shader
//! library as the external WESL package `lygia` (`import
//! lygia::generative::snoise::snoise2;`), see [`Modules`].
//!
//! The composed WGSL is then parsed and validated with `naga` (same major
//! version as the wgpu inside Bevy 0.20) so type errors in user code are
//! reported in the UI instead of only in the browser console.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use wesl::error::ResolveError;
use wesl::mangler::{EscapeMangler, Mangler};
use wesl::resolver::{PackageResolver, VirtualResolver};
use wesl::syntax::{ModulePath, PathOrigin};
use wesl::{CompileOptions, Compiler, Resolver};

use crate::heat;
use crate::pragmas::Pragmas;
use crate::tweak::{self, Constant, Hint, MAX_TWEAKS};

/// The WGSL uniform block, shared by [`SHADERTOY_MODULE`] and the app's
/// debugger overlay (`DEBUGGER_WGSL`) so the two cannot drift. Field order
/// must match the app's `ShaderToyUniform`; its `uniform_layout_matches_rust`
/// test checks every member's offset.
#[macro_export]
#[doc(hidden)]
macro_rules! shadertoy_uniforms_wgsl {
    () => {
        r#"struct ShaderToyUniforms {
    i_time: f32,
    i_time_delta: f32,
    i_frame: u32,
    // Heat mode only (`crate::heat`): the count at the palette's hot end.
    _heat_max: f32,
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
    // Live values of the Constants window (`crate::tweak`), one per slot.
    i_tweaks: array<vec4<f32>, 32>,
}
"#
    };
}
pub use crate::shadertoy_uniforms_wgsl;

/// The hidden `package::shadertoy` module: uniforms + accessors.
pub const SHADERTOY_MODULE: &str = concat!(
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
/// The rendered UI (toolbar, windows, everything) while the toolbar's UI
/// texture mode is on, else a transparent 1x1 texture. Only the image pass
/// sees it, and the shader's output replaces the UI on screen. Colors are
/// premultiplied by alpha. Read it through `i_ui_sample`, or texels with
/// `textureLoad` (row 0 is the top).
@group(0) @binding(7) var i_ui: texture_2d<f32>;
/// Bilinear, clamp-to-edge sampler for `i_ui`.
@group(0) @binding(8) var i_ui_sampler: sampler;

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
/// Live value of tweakable constant slot i; used by the generated loader,
/// not meant to be called by hand.
fn i_tweak(i: u32) -> vec4<f32> { return st_uniforms.i_tweaks[i]; }
/// The UI at `uv` (0..1, origin bottom-left like
/// `frag_coord / i_resolution().xy`), bilinear and premultiplied:
/// `ui.rgb + col * (1.0 - ui.a)` lays it over `col`. Transparent while the
/// UI texture mode is off.
fn i_ui_sample(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(i_ui, i_ui_sampler, vec2<f32>(uv.x, 1.0 - uv.y), 0.0);
}
"#
);

/// The bundled LYGIA modules (`vendor/lygia`, embedded by `build.rs`).
mod lygia {
    use wesl::package::{StaticPackage, StaticPackageModule};
    include!(concat!(env!("OUT_DIR"), "/lygia.rs"));
}

/// Every bundled LYGIA module as `(path relative to the package root,
/// source)`, e.g. `("generative/snoise.wesl", …)`: the files the analyzer's
/// workspace mirrors.
pub fn lygia_files() -> Vec<(String, &'static str)> {
    fn walk(
        module: &wesl::package::StaticPackageModule,
        dir: &str,
        out: &mut Vec<(String, &'static str)>,
    ) {
        for sub in module.submodules {
            if !sub.source.is_empty() {
                out.push((format!("{dir}{}.wesl", sub.name), sub.source));
            }
            walk(sub, &format!("{dir}{}/", sub.name), out);
        }
    }
    let mut files = Vec::new();
    walk(lygia::LYGIA.root, "", &mut files);
    files
}

/// The source of the bundled LYGIA module at `path` (`::`-separated below
/// the package root, e.g. `generative::snoise`).
pub fn lygia_source(path: &str) -> Option<&'static str> {
    let mut module = lygia::LYGIA.root;
    for name in path.split("::") {
        module = module.submodules.iter().find(|m| m.name == name).copied()?;
    }
    (!module.source.is_empty()).then_some(module.source)
}

/// Everything a compile can import: the app's in-memory `package::` modules
/// (`main`, `shadertoy`, `user`), the external packages, i.e. `lygia::`,
/// and WESL's virtual `constants::` module.
struct Modules<'a> {
    local: VirtualResolver<'a>,
    packages: PackageResolver,
    /// The `constants` module's source (see [`Pragmas`]).
    constants: String,
}

impl<'a> Modules<'a> {
    /// `shadertoy` plus `user` (and whatever `extra` modules) under
    /// `package::`, and the constants `pragmas` set.
    fn new(
        user: Cow<'a, str>,
        extra: impl IntoIterator<Item = (&'static str, Cow<'a, str>)>,
        pragmas: &Pragmas,
    ) -> Self {
        let mut local = VirtualResolver::new();
        let modules = [("shadertoy", SHADERTOY_MODULE.into()), ("user", user)];
        for (name, source) in modules.into_iter().chain(extra) {
            local.add_module(package_path(name), source);
        }
        let mut packages = PackageResolver::new();
        packages.add_package(&lygia::LYGIA);
        let constants = pragmas.constants_module();
        Self {
            local,
            packages,
            constants,
        }
    }
}

/// Whether `path` is in the `constants` module. Imported from inside a
/// package it may arrive as `lygia/constants`; every package shares the
/// one module, as with wesl's own `StandardResolver`.
fn is_constants(path: &ModulePath) -> bool {
    matches!(&path.origin, PathOrigin::Package(name)
        if name == "constants" || name.ends_with("/constants"))
}

impl Resolver for Modules<'_> {
    fn resolve_source<'b>(&'b self, path: &ModulePath) -> Result<Cow<'b, str>, ResolveError> {
        if is_constants(path) {
            Ok(Cow::Borrowed(self.constants.as_str()))
        } else if path.origin.is_package() {
            self.packages.resolve_source(path)
        } else {
            self.local.resolve_source(path)
        }
    }

    fn canonical_path(&self, path: &ModulePath) -> ModulePath {
        if is_constants(path) {
            ModulePath::new(
                PathOrigin::Package("constants".to_string()),
                path.components.clone(),
            )
        } else if path.origin.is_package() {
            self.packages.canonical_path(path)
        } else {
            path.clone()
        }
    }
}

/// WESL's defaults, minus visibility checks: LYGIA marks nothing `public`
/// (its own tooling does not enforce visibility), and with them on, wesl
/// refuses every import from it. The feature flags are the ones `pragmas`
/// turn on.
fn compile_options(pragmas: &Pragmas) -> CompileOptions {
    CompileOptions {
        visibility: false,
        features: pragmas.features(),
        ..Default::default()
    }
}

/// `package::{name}`.
fn package_path(name: &str) -> ModulePath {
    ModulePath::from_str(&format!("package::{name}")).expect("valid module path")
}

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
import package::user::st_load_tweaks;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // The Constants window's live values (see `tweak::instrument`).
    st_load_tweaks();
    // shadertoy's fragCoord has its origin bottom-left; wgpu's
    // @builtin(position) is top-left, so flip y to match shadertoy.
    let p = in.position.xy;
    return main_image(vec2<f32>(p.x, i_resolution().y - p.y));
}
"#;

/// Build the `package::main` module for a shader that defines the buffer
/// passes selected by `buffers` (bit per [`BUFFER_NAMES`] entry). Each buffer
/// gets its own fragment entry point, rendered into that channel's offscreen
/// target; undefined buffers get a stub entry (see the comment inside)
/// when `stubs` is set.
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
///
/// With `heat` on, the image entry shows the cost heatmap instead
/// ([`heat::MAIN_MODULE_HEAD`]); buffer entries stay as they are.
fn main_module_for(buffers: u8, heat: bool, stubs: bool) -> String {
    let mut wgsl = String::new();
    // Imports must come first
    for (i, name) in BUFFER_NAMES.iter().enumerate() {
        if buffers & (1 << i) != 0 {
            wgsl.push_str(&format!("import package::user::{name};\n"));
        }
    }
    if heat {
        wgsl.push_str(heat::MAIN_MODULE_HEAD);
        wgsl.push_str(&heat::palette_wgsl());
    } else {
        wgsl.push_str(MAIN_MODULE_HEAD);
    }
    for (i, name) in BUFFER_NAMES.iter().enumerate() {
        if !stubs && buffers & (1 << i) == 0 {
            continue;
        }
        let body = if buffers & (1 << i) != 0 {
            format!(
                "\x20   st_load_tweaks();\n\
                 \x20   // Buffer passes use texel coordinates (no y-flip).\n\
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
    /// The constants the Constants window can tweak live; slot = index.
    pub tweaks: Vec<Constant>,
    /// Literal constants that cannot be tweaked live because WGSL needs them
    /// as true constants (array sizes, other constants' values), or because
    /// there are more than [`MAX_TWEAKS`].
    pub fixed: Vec<String>,
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
///
/// Literal module-scope constants become live-tweakable (see [`tweak`]).
/// Any the rewrite cannot carry — used where WGSL needs a true constant in a
/// way [`tweak::const_context_names`] did not spot — are found by compiling
/// them one at a time and left as plain constants.
pub fn compile_user_shader(user_source: &str) -> Result<CompiledShader, CompileError> {
    compile_user_shader_with(user_source, false)
}

/// [`compile_user_shader`], with the image pass showing the cost heatmap
/// when `heat` is on (see [`heat`]). Errors are reported against the
/// plain source, as if heat mode were off.
pub fn compile_user_shader_with(
    user_source: &str,
    heat: bool,
) -> Result<CompiledShader, CompileError> {
    match compile_variant(user_source, heat, true) {
        Err(e) if heat => Err(compile_variant(user_source, false, true).err().unwrap_or(e)),
        result => result,
    }
}

/// [`compile_user_shader`] without the stub entry points of undefined
/// buffer passes, which only the app's shader slot reuse needs: a shader
/// without buffers then has `fragment` as its only entry point, which is
/// what exported crates (Bevy's stock fullscreen material) expect.
pub fn compile_for_export(user_source: &str) -> Result<CompiledShader, CompileError> {
    compile_variant(user_source, false, false)
}

fn compile_variant(
    user_source: &str,
    heat: bool,
    stubs: bool,
) -> Result<CompiledShader, CompileError> {
    let buffers = detect_buffers(user_source)?;

    let in_const_context = tweak::const_context_names(user_source);
    let mut fixed = Vec::new();
    let mut tweaks = Vec::new();
    for c in tweak::extract(user_source) {
        if c.hint == Hint::Off {
            continue;
        }
        if in_const_context.contains(&c.name) || tweaks.len() == MAX_TWEAKS {
            fixed.push(c.name);
        } else {
            tweaks.push(c);
        }
    }

    // Constants this same code (values aside) could not make live last
    // time: leave them fixed up front instead of searching again.
    let key = {
        let mut h = std::hash::DefaultHasher::new();
        (tweak::without_values(user_source, &tweaks), heat, stubs).hash(&mut h);
        h.finish()
    };
    let known = known_failing(key);
    if let Some(known) = &known {
        let (bad, ok): (Vec<Constant>, _) =
            tweaks.into_iter().partition(|c| known.contains(&c.name));
        fixed.extend(bad.into_iter().map(|c| c.name));
        tweaks = ok;
    }

    let compile =
        |tweaks: &[Constant]| compile_instrumented(user_source, buffers, stubs, tweaks, heat);
    let wgsl = match compile(&tweaks) {
        Ok(wgsl) => wgsl,
        Err(_) if !tweaks.is_empty() => {
            // Report the user's own errors against the plain source.
            compile(&[])?;
            let mut bad = Vec::new();
            failing(
                &tweaks,
                0..tweaks.len(),
                &mut |set| compile(set).is_ok(),
                &mut bad,
            );
            let mut failed: Vec<String> = Vec::new();
            for i in bad.into_iter().rev() {
                failed.push(tweaks.remove(i).name);
            }
            let wgsl = match compile(&tweaks) {
                Ok(wgsl) => wgsl,
                // Constants that fail only together: none stay live.
                Err(_) => {
                    failed.extend(tweaks.drain(..).map(|c| c.name));
                    compile(&[])?
                }
            };
            remember_failing(
                key,
                known
                    .unwrap_or_default()
                    .into_iter()
                    .chain(failed.iter().cloned()),
            );
            fixed.extend(failed);
            wgsl
        }
        Err(e) => return Err(e),
    };
    Ok(CompiledShader {
        wgsl,
        buffers,
        tweaks,
        fixed,
    })
}

/// Indices of the constants in `range` of `tweaks`, which together fail
/// to compile, that fail on their own: halve the range and search the
/// halves that fail. Each failing constant costs about two compiles per
/// halving, rather than one compile for every constant.
fn failing(
    tweaks: &[Constant],
    range: std::ops::Range<usize>,
    compiles: &mut impl FnMut(&[Constant]) -> bool,
    out: &mut Vec<usize>,
) {
    if range.len() == 1 {
        out.push(range.start);
        return;
    }
    let mid = range.start + range.len() / 2;
    for half in [range.start..mid, mid..range.end] {
        if !compiles(&tweaks[half.clone()]) {
            failing(tweaks, half, compiles, out);
        }
    }
}

/// Constants that could not be made live, per source (values aside, see
/// [`compile_variant`]). A handful of entries covers the editor and the
/// examples compiled alongside it.
static FAILING: std::sync::Mutex<Vec<(u64, Vec<String>)>> = std::sync::Mutex::new(Vec::new());
const FAILING_ENTRIES: usize = 16;

fn known_failing(key: u64) -> Option<Vec<String>> {
    let cache = FAILING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, names)| names.clone())
}

fn remember_failing(key: u64, names: impl Iterator<Item = String>) {
    let mut cache = FAILING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(k, _)| *k != key);
    if cache.len() == FAILING_ENTRIES {
        cache.remove(0);
    }
    cache.push((key, names.collect()));
}

/// Compile the user source with `tweaks` made live (see [`tweak::instrument`])
/// and, with `heat` on, its loops counted (see [`heat::instrument`]) into
/// a validated WGSL module, under the flags and constants its pragma lines
/// set ([`crate::pragmas`]).
fn compile_instrumented(
    user_source: &str,
    buffers: u8,
    stubs: bool,
    tweaks: &[Constant],
    heat: bool,
) -> Result<String, CompileError> {
    #[cfg(test)]
    tests::COMPILES.with(|n| n.set(n.get() + 1));
    let pragmas = Pragmas::parse(user_source);
    let mut user = tweak::instrument(user_source, tweaks);
    if heat {
        user = heat::instrument(&user);
    }
    let resolver = Modules::new(
        user.into(),
        [("main", main_module_for(buffers, heat, stubs).into())],
        &pragmas,
    );
    let compiler = Compiler::new_with_resolver(compile_options(&pragmas), resolver);
    let result = compiler
        .compile_module(&package_path("main"))
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
    Ok(wgsl)
}

/// Reject resource bindings that `package::shadertoy` does not declare.
///
/// The pipeline layout provides exactly the hidden module's resources. naga
/// accepts any binding, but wgpu rejects a pipeline whose shader uses a slot
/// the layout lacks or types differently, and Bevy's default render error
/// handler quits the app on that error, taking unsaved edits with it.
fn check_bindings(module: &naga::Module) -> Result<(), CompileError> {
    let env = package_path("shadertoy");
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
         package::shadertoy instead (i_sampler, i_channel_0..i_channel_3, i_ui; the \
         uniforms through i_time() and friends).",
        foreign.join(", ")
    )))
}

/// Compile `package::user` on its own and report which buffer passes it
/// defines, by looking for `fn buffer_a`..`buffer_d` in the output.
fn detect_buffers(user_source: &str) -> Result<u8, CompileError> {
    let pragmas = Pragmas::parse(user_source);
    let resolver = Modules::new(user_source.into(), [], &pragmas);

    // `keep_main` disables stripping *within* package::user, so unreferenced
    // `buffer_*` functions survive to be detected (the real compile in
    // `compile_user_shader` keeps stripping on: `main_image` is reachable
    // from the generated `package::main` entry points there).
    let options = CompileOptions {
        keep_main: true,
        ..compile_options(&pragmas)
    };
    let compiler = Compiler::new_with_resolver(options, resolver);
    let result = compiler
        .compile_module(&package_path("user"))
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

    /// Exports carry only the entry points the shader uses.
    #[test]
    fn export_has_no_stub_entries() {
        let src = "fn main_image(p: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }\n";
        let wgsl = compile_for_export(src).expect("should compile").wgsl;
        assert_eq!(wgsl.matches("@fragment").count(), 1, "{wgsl}");
        let wgsl = compile_user_shader(src).expect("should compile").wgsl;
        assert_eq!(wgsl.matches("@fragment").count(), 5, "{wgsl}");
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

    /// Literal constants become live uniform reads; ones WGSL needs as true
    /// constants stay constants, whether spotted up front (array size, used
    /// by another constant) or only by the compile (a `switch` case).
    #[test]
    fn literal_constants_become_tweakable() {
        let src = r#"
const N: i32 = 4;
const K: f32 = 0.5; // tweak: 0..2
const A: f32 = 1.0;
const B: f32 = A * 2.0;
const SEL: i32 = 1;
const TINT: vec3<f32> = vec3<f32>(1.0, 0.5, 0.25);
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var arr: array<f32, N>;
    var x = K + B + arr[0];
    switch i32(frag_coord.x) {
        case SEL: { x = 0.0; }
        default: {}
    }
    return vec4<f32>(TINT * x, 1.0);
}
"#;
        let compiled = compile_user_shader(src).expect("should compile");
        let live: Vec<&str> = compiled.tweaks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(live, ["K", "TINT"]);
        let mut fixed = compiled.fixed.clone();
        fixed.sort();
        assert_eq!(fixed, ["A", "N", "SEL"]);
        // Every entry point loads the live values before the user code runs.
        assert!(compiled.wgsl.contains("i_tweaks"), "{}", compiled.wgsl);
    }

    thread_local! {
        /// Compiles run on this thread, for [`fallback_compiles_little`].
        pub(super) static COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// A constant the rewrite cannot make live, among many, is found by
    /// halving rather than a compile per constant, and remembered: running
    /// the same code again, with other values, compiles once.
    #[test]
    fn fallback_compiles_little() {
        let shader = |k: f32| {
            let mut src = format!("const K: f32 = {k:?};\nconst SEL: i32 = 1;\n");
            for i in 0..30 {
                src += &format!("const C{i}: f32 = {i}.0;\n");
            }
            src += "fn main_image(p: vec2<f32>) -> vec4<f32> {\n    var x = K";
            for i in 0..30 {
                src += &format!(" + C{i}");
            }
            src += ";\n    switch i32(p.x) {\n        case SEL: { x = 0.0; }\n        default: {}\n    }\n";
            src += "    return vec4<f32>(x);\n}\n";
            src
        };
        let compiles = |src: &str| {
            COMPILES.with(|n| n.set(0));
            let compiled = compile_user_shader(src).expect("compiles");
            (compiled, COMPILES.with(std::cell::Cell::get))
        };

        let (compiled, first) = compiles(&shader(0.5));
        assert!(
            compiled.fixed.contains(&"SEL".to_string()),
            "{:?}",
            compiled.fixed
        );
        assert_eq!(compiled.tweaks.len(), 31);
        // One compile per constant would be 35.
        assert!(first <= 16, "{first} compiles");

        let (compiled, again) = compiles(&shader(0.75));
        assert!(compiled.fixed.contains(&"SEL".to_string()));
        assert_eq!(again, 1, "the second run searched again");
    }

    /// Errors in the user's code are reported against the plain source, not
    /// as a failure of the tweak rewrite.
    #[test]
    fn errors_ignore_the_tweak_rewrite() {
        let src = "const K: f32 = 0.5;\nfn main_image(p: vec2<f32>) -> vec4<f32> { return K; }\n";
        let msg = compile_user_shader(src).unwrap_err().to_string();
        assert!(!msg.contains("var<private>"), "{msg}");
        assert!(!msg.contains("st_load_tweaks"), "{msg}");
    }

    /// The bundled LYGIA resolves as the `lygia` package, including its
    /// own internal imports, and only what is used is linked in.
    #[test]
    fn imports_lygia() {
        let src = r#"
import package::shadertoy::i_time;
import lygia::generative::snoise::snoise3;
import lygia::color::palette::spectral::spectral;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let n = snoise3(vec3<f32>(frag_coord / 200.0, i_time()));
    return vec4<f32>(spectral(0.5 + 0.5 * n), 1.0);
}
"#;
        let wgsl = compile_user_shader(src).expect("should compile").wgsl;
        assert!(wgsl.contains("snoise3"), "{wgsl}");
        // snoise3 imports permute/mod289 from elsewhere in LYGIA.
        assert!(wgsl.contains("mod289"), "{wgsl}");
        assert!(!wgsl.contains("worley"), "unused modules leaked in");

        let missing = "import lygia::generative::nope::nope;\n\
                       fn main_image(p: vec2<f32>) -> vec4<f32> { return vec4<f32>(nope()); }\n";
        let msg = compile_user_shader(missing).unwrap_err().to_string();
        assert!(msg.contains("nope"), "{msg}");
    }

    /// A `// wesl-flags:` line switches a LYGIA module's `@if` branch, and
    /// user code can test flags of its own.
    #[test]
    fn flags_select_lygia_code() {
        let src = r#"
import lygia::color::space::rgb2yuv::rgb2yuv;

@if(BRIGHT)
const GAIN: f32 = 2.5; // tweak: off
@else
const GAIN: f32 = 1.5; // tweak: off

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(rgb2yuv(vec3<f32>(frag_coord.x * 0.001)) * GAIN, 1.0);
}
"#;
        let hdtv = compile_user_shader(src)
            .expect("compiles without flags")
            .wgsl;
        let flagged = format!("// wesl-flags: YUV_SDTV, BRIGHT\n{src}");
        let sdtv = compile_user_shader(&flagged)
            .expect("compiles with flags")
            .wgsl;
        assert_ne!(hdtv, sdtv);
        // BT.709 luma weights without the flag, BT.601 with it.
        assert!(hdtv.contains("0.2126") && !hdtv.contains("0.299"), "{hdtv}");
        assert!(sdtv.contains("0.299") && !sdtv.contains("0.2126"), "{sdtv}");
        assert!(hdtv.contains("1.5") && !hdtv.contains("2.5"), "{hdtv}");
        assert!(sdtv.contains("2.5") && !sdtv.contains("1.5"), "{sdtv}");
    }

    /// `// wesl-const:` feeds the `constants::` module, and turns on the
    /// same-named flag that guards its import in LYGIA.
    #[test]
    fn constants_reach_lygia() {
        let src = r#"
import package::shadertoy::i_frame;
import lygia::distort::marble::marbleBake1;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(f32(marbleBake1(f32(i_frame()))));
}
"#;
        let default = compile_user_shader(src).expect("compiles").wgsl;
        assert!(default.contains("60.0"), "{default}");
        let set = format!("// wesl-const: MARBLE_PERIOD = 90.0\n{src}");
        let wgsl = compile_user_shader(&set)
            .expect("compiles with the constant")
            .wgsl;
        assert!(wgsl.contains("90.0") && !wgsl.contains("60.0"), "{wgsl}");

        // The user's own code can read constants too, of any WGSL type.
        let own = "// wesl-const: TINT = vec3f(0.25, 0.5, 1.0)\n\
                   import constants::TINT;\n\
                   fn main_image(p: vec2<f32>) -> vec4<f32> { return vec4<f32>(TINT, 1.0); }\n";
        let wgsl = compile_user_shader(own).expect("compiles").wgsl;
        assert!(wgsl.contains("0.25"), "{wgsl}");
        // Without a value the import fails, and says what is missing.
        let missing = own.lines().skip(1).collect::<Vec<_>>().join("\n");
        let msg = compile_user_shader(&missing).unwrap_err().to_string();
        assert!(msg.contains("TINT"), "{msg}");
    }

    #[test]
    fn lygia_files_mirror_the_package() {
        let files = lygia_files();
        assert!(files.len() > 300, "{}", files.len());
        let snoise = files
            .iter()
            .find(|(path, _)| path == "generative/snoise.wesl")
            .expect("snoise module");
        assert!(snoise.1.contains("fn snoise3"));
        // A module that is both a file and a directory keeps both.
        assert!(files.iter().any(|(p, _)| p == "math/quat.wesl"));
        assert!(files.iter().any(|(p, _)| p.starts_with("math/quat/")));
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
