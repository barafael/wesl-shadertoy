//! Cross-backend diffing for wesl-shadertoy: a naga fuzzer in disguise.
//!
//! The app's shaders reach the screen through naga translations — SPIR-V on
//! Vulkan, GLSL ES on WebGL2 (and native GL), MSL and HLSL elsewhere — so a
//! translation bug shows up as a picture that depends on the backend. This
//! crate composes a shader exactly like the app ([`wesl_shadertoy::compile`]),
//! renders it on every wgpu adapter the machine has ([`gpu`]), and diffs each
//! render against the first one, Vulkan when present ([`image`]). It also
//! writes every naga backend's output directly ([`translate`]), which covers
//! MSL and HLSL without the hardware, and generates random well-typed shaders
//! to feed both ([`fuzz`]).
//!
//! Run it as `cargo test -p backend-diff` (asserts; skips the rendering
//! checks without two adapters) or `cargo run -p backend-diff -- [--fuzz N]`
//! (prints a table). Renders of shaders that differ land in
//! `target/backend-diff/`.

pub mod fuzz;
pub mod gpu;
pub mod image;
pub mod translate;

use std::path::PathBuf;

use wesl_shadertoy::compile::compile_user_shader;

use crate::gpu::{Gpu, Scene};
use crate::image::{Diff, Image};

/// A shader under test: an example or a fuzzed one.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    /// The `package::user` source, as the editor would hold it.
    pub source: String,
}

impl Case {
    /// Every bundled example.
    pub fn examples() -> Vec<Case> {
        wesl_shadertoy::examples::ALL
            .iter()
            .map(|e| Case {
                name: e.name.to_string(),
                source: e.code.to_string(),
            })
            .collect()
    }

    /// The fuzz shader for `seed` ([`fuzz::shader`]).
    pub fn fuzz(seed: u64) -> Case {
        Case {
            name: format!("fuzz-{seed}"),
            source: fuzz::shader(seed),
        }
    }

    /// The integer fuzz shader for `seed` ([`fuzz::int_shader`]).
    pub fn fuzz_int(seed: u64) -> Case {
        Case {
            name: format!("ifuzz-{seed}"),
            source: fuzz::int_shader(seed),
        }
    }

    /// File-name-safe form of the name.
    pub fn slug(&self) -> String {
        self.name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    }
}

/// One adapter's render of a case, compared with the reference adapter's.
pub struct Comparison {
    /// [`Gpu::label`] of the compared adapter.
    pub label: String,
    /// The diff, or why there is none (a render failed on either side).
    pub diff: Result<Diff, String>,
}

impl Comparison {
    pub fn matches(&self) -> bool {
        self.diff.as_ref().is_ok_and(Diff::matches)
    }
}

/// Compile `case` and render it on every adapter in `gpus`, comparing each
/// with `gpus[0]`. Cases that differ get their renders, the diff and the
/// source written to [`out_dir`]. `Err` when the case does not compile (its
/// source is saved too, so a fuzz shader that trips the compiler is kept).
pub fn compare(case: &Case, gpus: &[Gpu], scene: &Scene) -> Result<Vec<Comparison>, String> {
    let shader = compile_user_shader(&case.source).map_err(|e| {
        save_source(case);
        e.to_string()
    })?;
    let renders: Vec<Result<Image, String>> =
        gpus.iter().map(|g| g.render(&shader, scene)).collect();
    let Some((reference, others)) = renders.split_first() else {
        return Ok(Vec::new());
    };
    let mut comparisons = Vec::new();
    for (gpu, render) in gpus[1..].iter().zip(others) {
        let diff = match (reference, render) {
            (Ok(a), Ok(b)) => Ok(Diff::new(a, b)),
            (Err(e), _) => Err(format!("{} failed: {e}", gpus[0].label)),
            (_, Err(e)) => Err(format!("{} failed: {e}", gpu.label)),
        };
        let comparison = Comparison {
            label: gpu.label.clone(),
            diff,
        };
        if !comparison.matches() {
            save(case, &gpus[0], reference, gpu, render, &comparison);
        }
        comparisons.push(comparison);
    }
    Ok(comparisons)
}

/// Write what there is of a mismatch to [`out_dir`]: both renders, the
/// amplified diff and the shader (best effort; a report without the files
/// is still a report).
fn save(
    case: &Case,
    reference: &Gpu,
    reference_render: &Result<Image, String>,
    gpu: &Gpu,
    render: &Result<Image, String>,
    comparison: &Comparison,
) {
    let Some(dir) = save_source(case) else {
        return;
    };
    let slug = case.slug();
    for (g, r) in [(reference, reference_render), (gpu, render)] {
        if let Ok(image) = r {
            let _ = image.write_png(&dir.join(format!("{slug}.{}.png", g.label)));
        }
    }
    if let Ok(diff) = &comparison.diff {
        let path = dir.join(format!(
            "{slug}.{}-vs-{}.diff.png",
            reference.label, gpu.label
        ));
        let _ = diff.image.write_png(&path);
    }
}

/// Write `case`'s source to [`out_dir`], which is returned if it exists.
fn save_source(case: &Case) -> Option<PathBuf> {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let _ = std::fs::write(dir.join(format!("{}.wesl", case.slug())), &case.source);
    Some(dir)
}

/// `target/backend-diff/` (under `CARGO_TARGET_DIR` when set).
pub fn out_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // crates/backend-diff → the workspace root.
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2);
            root.expect("crate sits two levels below the workspace")
                .join("target")
        })
        .join("backend-diff")
}
