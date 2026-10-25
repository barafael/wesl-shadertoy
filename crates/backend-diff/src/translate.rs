//! naga's shader writers, run directly on a composed module.
//!
//! wgpu only translates for the backends it runs on, so a Linux machine never
//! exercises the MSL or HLSL writers. Writing every backend's output here
//! catches writer errors and panics without the hardware. Where the
//! reference tools are installed (`glslangValidator`, `spirv-val`), the GLSL
//! and SPIR-V output is also checked by them, since a writer can succeed and
//! still emit code no driver accepts.

use std::io::Write as _;
use std::process::{Command, Stdio};

/// The targets, in report order.
pub const TARGETS: [&str; 4] = ["glsl", "msl", "hlsl", "spv"];

/// One target's verdict: `Ok` with a note (empty, or which validator ran),
/// or the error.
pub type Outcome = Result<&'static str, String>;

/// Parse and validate `wgsl`, then write it for every [`TARGETS`] entry.
/// `Err` when the WGSL itself does not get that far.
pub fn translate(wgsl: &str) -> Result<[Outcome; 4], String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(wgsl))?;
    let caught = |f: &dyn Fn() -> Outcome| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .unwrap_or_else(|panic| Err(format!("writer panicked: {}", panic_message(&panic))))
    };
    Ok([
        caught(&|| glsl(&module, &info)),
        caught(&|| msl(&module, &info)),
        caught(&|| hlsl(&module, &info)),
        caught(&|| spv(&module, &info)),
    ])
}

/// GLSL ES 3.00 as WebGL2 sees it, one fragment entry point at a time (the
/// GLSL writer emits a single entry point per shader).
fn glsl(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Outcome {
    use naga::back::glsl;
    let options = glsl::Options {
        version: glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        ..Default::default()
    };
    let mut note = "";
    for entry in &module.entry_points {
        let pipeline = glsl::PipelineOptions {
            shader_stage: entry.stage,
            entry_point: entry.name.clone(),
            multiview: None,
        };
        let mut out = String::new();
        glsl::Writer::new(
            &mut out,
            module,
            info,
            &options,
            &pipeline,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .and_then(|mut w| w.write())
        .map_err(|e| format!("{}: {e}", entry.name))?;
        note = external(
            "glslangValidator",
            &["--stdin", "-S", "frag"],
            out.as_bytes(),
        )
        .map_err(|e| format!("{}: glslangValidator: {e}", entry.name))?;
    }
    Ok(note)
}

fn msl(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Outcome {
    use naga::back::msl;
    let options = msl::Options {
        lang_version: (2, 0),
        ..Default::default()
    };
    msl::write_string(module, info, &options, &Default::default()).map_err(|e| e.to_string())?;
    Ok("")
}

fn hlsl(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Outcome {
    use naga::back::hlsl;
    let options = hlsl::Options {
        shader_model: hlsl::ShaderModel::V6_0,
        ..Default::default()
    };
    let mut out = String::new();
    hlsl::Writer::new(&mut out, &options, &Default::default())
        .write(module, info, None)
        .map_err(|e| e.to_string())?;
    Ok("")
}

fn spv(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Outcome {
    use naga::back::spv;
    let words =
        spv::write_vec(module, info, &Default::default(), None).map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    external("spirv-val", &["-"], &bytes).map_err(|e| format!("spirv-val: {e}"))
}

/// Feed `input` to `tool` on stdin. `Ok("")` when the tool is not installed,
/// `Ok(tool)` when it accepted the input, `Err` with its output otherwise.
fn external(tool: &'static str, args: &[&str], input: &[u8]) -> Outcome {
    let Ok(mut child) = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Ok("");
    };
    // A failed write means the tool exited early; its status says why.
    let _ = child.stdin.take().map(|mut stdin| stdin.write_all(input));
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(tool);
    }
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    Err(text.trim().to_string())
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string payload".into())
}
