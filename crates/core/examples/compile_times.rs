//! How long each bundled example takes to compile, as the app does on
//! Ctrl+Enter (`compile_user_shader`): the first run (cold: constants that
//! cannot be live are searched for) and the best of four more, slowest
//! first.
//!
//! `cargo run --release -p wesl-shadertoy-core --example compile_times`

use std::time::Instant;

use wesl_shadertoy_core::compile::compile_user_shader;
use wesl_shadertoy_core::examples::ALL;

fn main() {
    let time = |code: &str| {
        let t = Instant::now();
        let wgsl = compile_user_shader(code).expect("compiles").wgsl;
        (t.elapsed().as_secs_f64() * 1000.0, wgsl.len())
    };
    let mut rows: Vec<(f64, f64, &str, usize)> = ALL
        .iter()
        .map(|e| {
            let (first, len) = time(e.code);
            let best = (0..4).map(|_| time(e.code).0).fold(f64::MAX, f64::min);
            (best, first, e.name, len)
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("   first     best     WGSL  example");
    for (best, first, name, len) in &rows {
        println!("{first:8.1} {best:8.1} {len:>8}  {name}");
    }
    let total: f64 = rows.iter().map(|r| r.0).sum();
    println!(
        "best: median {:.1} ms, total {total:.0} ms (all in ms)",
        rows[rows.len() / 2].0
    );
}
