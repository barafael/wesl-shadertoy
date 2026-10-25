//! `cargo run -p backend-diff -- [--fuzz N] [--seed S] [--only NAME] [FILE.wesl…]`
//!
//! Prints the adapters found, then one row per case and adapter: the largest
//! channel difference against the reference adapter, the share of pixels
//! over the tolerance, and the naga writers' verdicts. Files given on the
//! command line are compared instead of the examples. Exits with 1 when
//! anything differs or fails to translate.

use backend_diff::gpu::{Gpu, Scene};
use backend_diff::image::{MAX_DIFFERING_SHARE, PIXEL_TOLERANCE};
use backend_diff::translate::{self, TARGETS};
use backend_diff::{Case, compare, out_dir};
use wesl_shadertoy::compile::compile_user_shader;

fn main() {
    let mut fuzz = 0u64;
    let mut seed = 0u64;
    let mut only: Option<String> = None;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .unwrap_or_else(|| usage(&format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "--fuzz" => {
                fuzz = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--fuzz takes a count"))
            }
            "--seed" => {
                seed = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--seed takes a number"))
            }
            "--only" => only = Some(value()),
            _ if arg.starts_with("--") => usage(&format!("unknown argument {arg:?}")),
            _ => files.push(arg),
        }
    }

    let mut cases = if files.is_empty() {
        Case::examples()
    } else {
        files.iter().map(|path| file_case(path)).collect()
    };
    cases.extend((seed..seed + fuzz).map(Case::fuzz));
    if let Some(only) = &only {
        cases.retain(|c| c.name.eq_ignore_ascii_case(only));
        if cases.is_empty() {
            usage(&format!("no case named {only:?}"));
        }
    }

    let (gpus, skipped) = Gpu::all(wgpu::Backends::all());
    for (i, gpu) in gpus.iter().enumerate() {
        let role = if i == 0 { "reference" } else { "compared" };
        println!("{:<9} {:<8} {}", role, gpu.label, gpu.description);
    }
    for s in &skipped {
        println!("skipped   {s}");
    }
    if gpus.len() < 2 {
        println!("(fewer than two adapters: rendering is not compared, only translated)");
    }
    println!(
        "tolerance: {PIXEL_TOLERANCE}/255 per channel, at most {:.1}% of pixels\n",
        MAX_DIFFERING_SHARE * 100.0
    );

    let scene = Scene::default();
    println!(
        "{:<22} {:<8} {:>5} {:>8}  {:<8}  {}",
        "case",
        "adapter",
        "max",
        "over",
        "verdict",
        TARGETS.join(" ")
    );
    let mut failures = 0;
    for case in &cases {
        let comparisons = match compare(case, &gpus, &scene) {
            Ok(c) => c,
            Err(e) => {
                failures += 1;
                println!("{:<22} compile failed: {e}", case.name);
                continue;
            }
        };
        let wgsl = compile_user_shader(&case.source)
            .expect("compiled for the comparison")
            .wgsl;
        // The writers' verdicts, and their errors to print under the rows.
        let (naga, errors): (String, Vec<String>) = match translate::translate(&wgsl) {
            Ok(outcomes) => {
                let cells = outcomes
                    .iter()
                    .map(|o| if o.is_ok() { "ok  " } else { "FAIL" });
                let errors = TARGETS
                    .iter()
                    .zip(&outcomes)
                    .filter_map(|(target, o)| Some(format!("{target}: {}", o.as_ref().err()?)))
                    .collect();
                (cells.collect::<Vec<_>>().join(" "), errors)
            }
            Err(e) => ("invalid".into(), vec![e]),
        };
        failures += errors.len();
        if comparisons.is_empty() {
            println!(
                "{:<22} {:<8} {:>5} {:>8}  {:<8}  {naga}",
                case.name, "-", "-", "-", "-"
            );
        }
        for c in &comparisons {
            let (max, over, verdict) = match &c.diff {
                Ok(d) => (
                    d.max.to_string(),
                    format!("{:.2}%", d.share() * 100.0),
                    if d.matches() { "same" } else { "DIFFERS" },
                ),
                Err(_) => ("-".into(), "-".into(), "ERROR"),
            };
            failures += usize::from(!c.matches());
            println!(
                "{:<22} {:<8} {max:>5} {over:>8}  {verdict:<8}  {naga}",
                case.name, c.label
            );
            if let Err(e) = &c.diff {
                println!("  {e}");
            }
        }
        for e in errors {
            println!("  {e}");
        }
    }
    if failures > 0 {
        println!(
            "\n{failures} problem(s); renders and sources of failing cases are in {}",
            out_dir().display()
        );
        std::process::exit(1);
    }
}

/// A `.wesl` file as a case named after its stem.
fn file_case(path: &str) -> Case {
    let source = std::fs::read_to_string(path).unwrap_or_else(|e| usage(&format!("{path}: {e}")));
    let path = std::path::Path::new(path);
    Case {
        name: path
            .file_stem()
            .map_or_else(|| "file".into(), |s| s.to_string_lossy().into_owned()),
        source,
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("{problem}\nusage: backend-diff [--fuzz N] [--seed S] [--only NAME] [FILE.wesl…]");
    std::process::exit(2);
}
