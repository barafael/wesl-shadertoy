//! Upstream bugs this crate found, reduced to plain WGSL and checked against
//! naga alone. Ignored while the bug stands; `cargo test -p backend-diff --
//! --ignored` tells when an upgrade fixes one.

/// Parse and validate `wgsl` with naga's defaults.
fn naga_accepts(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(wgsl))?;
    Ok(())
}

/// `mix` and `smoothstep` of abstract-float arguments are constant
/// expressions that convert to `f32`, but naga 30 does not const-evaluate
/// them: with a type annotation validation fails with "Abstract types may
/// only appear in constant expressions", without one the parser reports
/// "Subexpression(s) are not constant". `clamp`, `pow`, `dot`, `length`,
/// `fract`, `cos` and `select` in the same position are fine. Found by the
/// fuzzer (seed 1 with abstract literals).
#[test]
#[ignore = "naga 30 rejects const mix/smoothstep of abstract floats"]
fn abstract_mix_and_smoothstep_are_const_evaluated() {
    for expr in [
        "let v: f32 = smoothstep(0.0, 1.0, 0.5);",
        "let v = smoothstep(0.0, 1.0, 0.5);",
        "let v: f32 = mix(0.0, 1.0, 0.5);",
        "let v: vec2<f32> = smoothstep(vec2(0.0), vec2(1.0), vec2(0.5));",
    ] {
        let wgsl = format!(
            "@fragment fn main() -> @location(0) vec4<f32> {{\n    {expr}\n    return vec4<f32>(0.0);\n}}\n"
        );
        if let Err(e) = naga_accepts(&wgsl) {
            panic!("{expr}\n{e}");
        }
    }
}

/// The same position with builtins naga does evaluate, so the test above
/// fails for the reason it names.
#[test]
fn abstract_clamp_and_friends_are_const_evaluated() {
    for expr in [
        "let v: f32 = clamp(2.0, 0.0, 1.0);",
        "let v: f32 = pow(2.0, 0.5);",
        "let v: f32 = dot(vec2(0.3, 0.4), vec2(1.0, 2.0));",
    ] {
        let wgsl = format!(
            "@fragment fn main() -> @location(0) vec4<f32> {{\n    {expr}\n    return vec4<f32>(0.0);\n}}\n"
        );
        if let Err(e) = naga_accepts(&wgsl) {
            panic!("{expr}\n{e}");
        }
    }
}
