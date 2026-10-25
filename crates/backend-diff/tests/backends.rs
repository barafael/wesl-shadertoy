//! The examples and a batch of fuzz shaders, through naga's writers and
//! through every adapter on the machine. The rendering checks need two
//! adapters (Vulkan and GL on a typical Linux box) and skip, saying why,
//! without them; the translation checks need no GPU.

use backend_diff::gpu::{Gpu, Scene};
use backend_diff::translate::{self, TARGETS};
use backend_diff::{Case, compare, out_dir};
use wesl_shadertoy::compile::compile_user_shader;

/// Fuzz seeds the tests render (the binary's `--fuzz N` runs more).
const FUZZ_SEEDS: std::ops::Range<u64> = 0..48;

/// Examples whose picture legitimately depends on the backend, reported but
/// not failed. Both hash with `fract(sin(dot(p, K)) * 43758.5453)`, and
/// drivers may contract `dot` into a fused multiply-add in either operand
/// order (RADV's SPIR-V path and radeonsi's GLSL path pick opposite ones):
/// the last bit of the dot product differs, and `sin(…) * 43758.5` followed
/// by `fract` turns that bit into a different random value per cell.
/// Allowed by both WGSL and Vulkan precision rules, so not a translation bug.
/// (Fire and Voronoi used to be listed too, until their noise moved to
/// LYGIA's, which doesn't hash this way.)
const BACKEND_DEPENDENT: &[&str] = &["Elevated", "Seascape"];

/// Translate every case for every target; the failures, one line each.
fn translation_failures(cases: &[Case]) -> Vec<String> {
    let mut failures = Vec::new();
    for case in cases {
        let wgsl = compile_user_shader(&case.source)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name))
            .wgsl;
        match translate::translate(&wgsl) {
            Ok(outcomes) => {
                for (target, outcome) in TARGETS.iter().zip(outcomes) {
                    if let Err(e) = outcome {
                        failures.push(format!("{} → {target}: {e}", case.name));
                    }
                }
            }
            Err(e) => failures.push(format!("{}: {e}", case.name)),
        }
    }
    failures
}

#[test]
fn examples_translate_to_every_target() {
    let failures = translation_failures(&Case::examples());
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn fuzz_shaders_translate_to_every_target() {
    let cases: Vec<Case> = FUZZ_SEEDS.map(Case::fuzz).collect();
    let failures = translation_failures(&cases);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Render `cases` on every adapter and fail on any that differs from the
/// reference adapter's render, except that a difference in a `tolerated`
/// case is only printed (a failed render is not tolerated).
fn assert_backends_agree(cases: &[Case], tolerated: &[&str]) {
    let (gpus, skipped) = Gpu::all(wgpu::Backends::all());
    for s in &skipped {
        println!("adapter skipped: {s}");
    }
    if gpus.len() < 2 {
        let found: Vec<&str> = gpus.iter().map(|g| g.description.as_str()).collect();
        println!("SKIPPED: needs two adapters to compare, found {found:?}");
        return;
    }
    let scene = Scene::default();
    let mut failures = Vec::new();
    for case in cases {
        for c in compare(case, &gpus, &scene).unwrap_or_else(|e| panic!("{}: {e}", case.name)) {
            match &c.diff {
                Ok(d) if d.matches() => {}
                Ok(d) if tolerated.contains(&case.name.as_str()) => println!(
                    "{} on {}: differs as expected (max {}, {:.2}% of pixels)",
                    case.name,
                    c.label,
                    d.max,
                    d.share() * 100.0
                ),
                Ok(d) => failures.push(format!(
                    "{} on {} vs {}: max {}, {:.2}% of pixels over the tolerance",
                    case.name,
                    c.label,
                    gpus[0].label,
                    d.max,
                    d.share() * 100.0
                )),
                Err(e) => failures.push(format!("{} on {}: {e}", case.name, c.label)),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}\n(renders and diffs in {})",
        failures.join("\n"),
        out_dir().display()
    );
}

#[test]
fn examples_render_alike_on_every_backend() {
    assert_backends_agree(&Case::examples(), BACKEND_DEPENDENT);
}

#[test]
fn fuzz_shaders_render_alike_on_every_backend() {
    let cases: Vec<Case> = FUZZ_SEEDS.map(Case::fuzz).collect();
    assert_backends_agree(&cases, &[]);
}
