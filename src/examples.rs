//! The built-in examples, one file each in `examples/`, written in the
//! wesl-shadertoy dialect: the editor text is the `package::user` module, so
//! each example starts with the `import package::shadertoy::...` lines that
//! pull in the uniforms it uses.
//!
//! `build.rs` embeds every `examples/*.wesl` file, sorted by name. A file
//! starts with a comment block that names and credits it:
//!
//! ```text
//! // Name — one-line summary.
//! // author: who wrote it
//! // license: SPDX-ish license name
//! // source: https://… (optional: where it was ported from)
//! // tags: 2d, noise (optional; buffers, mouse and lygia are added
//! //       automatically when the code uses them)
//! ```
//!
//! Adding an example is dropping a file into `examples/`; the tests below
//! check that it compiles and offers live constants.

pub struct Example {
    pub name: &'static str,
    pub summary: &'static str,
    pub author: &'static str,
    pub license: &'static str,
    /// Where it was ported from (empty for originals).
    pub source: &'static str,
    pub tags: &'static [&'static str],
    pub code: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/examples.rs"));

/// The example a fresh session starts from when "random example" is off.
pub fn default() -> &'static Example {
    by_name("Plasma").unwrap_or(&ALL[0])
}

/// Case-insensitive lookup by display name.
pub fn by_name(name: &str) -> Option<&'static Example> {
    ALL.iter().find(|e| e.name.eq_ignore_ascii_case(name))
}

/// All tags in use, sorted, for the browser's filter.
pub fn tags() -> Vec<&'static str> {
    let mut tags: Vec<&str> = ALL.iter().flat_map(|e| e.tags.iter().copied()).collect();
    tags.sort_unstable();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_user_shader;

    /// Every example offers knobs in the Constants window, and none of its
    /// literal constants silently fails to be live (those are marked
    /// `// tweak: off` instead).
    #[test]
    fn examples_have_live_constants() {
        for example in ALL {
            let compiled = compile_user_shader(example.code).expect("example compiles");
            assert!(
                !compiled.tweaks.is_empty(),
                "{}: no live constants",
                example.name
            );
            assert!(
                compiled.fixed.is_empty(),
                "{}: constants left fixed: {:?}",
                example.name,
                compiled.fixed
            );
        }
    }

    #[test]
    fn all_examples_compile() {
        for example in ALL {
            compile_user_shader(example.code)
                .unwrap_or_else(|e| panic!("example `{}` failed: {e}", example.name));
        }
    }

    #[test]
    fn metadata_is_parsed() {
        let plasma = by_name("plasma").expect("Plasma example");
        assert_eq!(
            plasma.summary,
            "the classic cosine palette, driven by i_time."
        );
        assert_eq!(plasma.license, "MIT OR Apache-2.0");
        assert!(plasma.tags.contains(&"2d"));
        // Implied tags come from the code.
        let fluid = by_name("Fluid").expect("Fluid example");
        assert!(fluid.tags.contains(&"buffers") && fluid.tags.contains(&"mouse"));
        assert!(!fluid.source.is_empty());
        // Names are unique, since the browser and `--example` look them up.
        for (i, a) in ALL.iter().enumerate() {
            assert!(ALL[..i].iter().all(|b| b.name != a.name), "{}", a.name);
        }
    }
}
