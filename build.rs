//! Generates what the app embeds besides the compiler's own sources (those
//! are `wesl-shadertoy-core`'s, see its build script):
//!
//! * `vendor/lygia/**/*.wesl` → `$OUT_DIR/lygia_docs.rs`, the index behind
//!   the LYGIA window (`crate::library::MODULES`): each module's doc block,
//!   its declarations as written in WGSL, the feature flags and
//!   `constants::` it reads and the modules it imports.
//! * every example, compiled → `$OUT_DIR/compiled_examples.rs`, the WGSL
//!   behind the Examples window's cards (`crate::browser`), so the browser
//!   never compiles them at run time (tens of milliseconds each in wasm). An
//!   example that does not compile fails the build.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// Import, flag and constant scans, shared with the app.
#[path = "src/wesl_scan.rs"]
mod wesl_scan;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo::rerun-if-changed=build.rs");

    let lygia = root.join("vendor/lygia");
    println!("cargo::rerun-if-changed={}", lygia.display());
    std::fs::write(out.join("lygia_docs.rs"), lygia_docs(&lygia)).unwrap();

    // The core crate embeds the examples, so a changed example rebuilds it
    // and reruns this script; the line below says so explicitly.
    println!(
        "cargo::rerun-if-changed={}",
        root.join("examples").display()
    );
    std::fs::write(out.join("compiled_examples.rs"), compiled_examples()).unwrap();
}

/// `COMPILED`: every example composed and validated, in
/// `examples::ALL` order, with its buffer mask and its live constants'
/// default values.
fn compiled_examples() -> String {
    use wesl_shadertoy_core::compile::compile_user_shader;
    use wesl_shadertoy_core::examples::ALL;
    let mut out = String::from("static COMPILED: &[CompiledExample] = &[\n");
    for example in ALL {
        let compiled = compile_user_shader(example.code)
            .unwrap_or_else(|e| panic!("example {} does not compile: {e}", example.name));
        let tweaks: Vec<String> = compiled
            .tweaks
            .iter()
            .map(|c| format!("{:?}", c.value))
            .collect();
        let _ = writeln!(
            out,
            "    CompiledExample {{ wgsl: {:?}, buffers: {}, tweaks: &[{}] }},",
            compiled.wgsl,
            compiled.buffers,
            tweaks.join(", ")
        );
    }
    out.push_str("];\n");
    out
}

/// `MODULES`: one `ModuleDoc` literal per LYGIA file, sorted by path.
fn lygia_docs(dir: &Path) -> String {
    let mut files = Vec::new();
    wesl_files(dir, "", &mut files);
    files.sort();
    let paths: Vec<String> = files.iter().map(|(path, _)| path.clone()).collect();
    let mut out = String::from("pub static MODULES: &[ModuleDoc] = &[\n");
    for (path, file) in &files {
        let text = std::fs::read_to_string(file).unwrap();
        out.push_str(&module_doc(path, &text, &paths));
    }
    out.push_str("];\n");
    out
}

/// Every `.wesl` file below `dir` as `(module path, file)`, the module path
/// `::`-separated below the package root (`generative::snoise`).
fn wesl_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    for entry in sorted_entries(dir) {
        let Some(stem) = entry.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let path = format!("{prefix}{stem}");
        if entry.is_dir() {
            wesl_files(&entry, &format!("{path}::"), out);
        } else if entry.extension().is_some_and(|e| e == "wesl") {
            out.push((path, entry));
        }
    }
}

/// The `ModuleDoc` literal for module `path` with source `text`; `modules`
/// lists every module path, to tell which part of an import names the
/// module.
fn module_doc(path: &str, text: &str, modules: &[String]) -> String {
    let doc = DocBlock::parse(text);
    let mut imports: Vec<String> = Vec::new();
    for import in wesl_scan::package_paths(text, "lygia") {
        // The longest prefix that is a module: `color::blend::darken` of
        // `color::blend::darken::blendDarken`.
        let module = (1..=import.len())
            .rev()
            .map(|n| import[..n].join("::"))
            .find(|m| modules.contains(m));
        if let Some(module) = module
            && module != path
            && !imports.contains(&module)
        {
            imports.push(module);
        }
    }
    let list = |items: &[String]| {
        items
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let notes = doc
        .flag_notes
        .iter()
        .map(|(cond, note)| format!("({cond:?}, {note:?})"))
        .collect::<Vec<_>>()
        .join(", ");
    let items = declarations(text)
        .iter()
        .map(|d| {
            format!(
                "\n            Item {{ kind: ItemKind::{}, name: {:?}, signature: {:?}, doc: {:?} }},",
                d.kind, d.name, d.signature, d.doc
            )
        })
        .collect::<String>();
    format!(
        "    ModuleDoc {{\n        path: {path:?},\n        description: {:?},\n        \
         contributors: &[{}],\n        uses: &[{}],\n        license: &[{}],\n        \
         flag_notes: &[{notes}],\n        items: &[{items}\n        ],\n        \
         flags: &[{}],\n        constants: &[{}],\n        imports: &[{}],\n    }},\n",
        doc.description,
        list(&doc.contributors),
        list(&doc.uses),
        list(&doc.license),
        list(&wesl_scan::flag_names(text)),
        list(&wesl_scan::constant_names(text)),
        list(&imports),
    )
}

/// What a LYGIA file's leading `/* … */` block says about it. The block is
/// YAML in all but name; this reads the subset LYGIA uses (plain, quoted and
/// `|` scalars, and lists).
#[derive(Default)]
struct DocBlock {
    description: String,
    contributors: Vec<String>,
    uses: Vec<String>,
    license: Vec<String>,
    /// `(condition, what it does)`, from the `- @if(FLAG): …` notes in any
    /// field and the `options:` list (`- NAME: …`).
    flag_notes: Vec<(String, String)>,
}

impl DocBlock {
    fn parse(text: &str) -> Self {
        let mut doc = DocBlock::default();
        let Some(block) = text
            .split("/*")
            .skip(1)
            .filter_map(|rest| rest.split_once("*/").map(|(body, _)| body))
            .find(|body| body.contains("description:") || body.contains("contributors:"))
        else {
            return doc;
        };
        // Top-level `key: value` lines start a field; indented lines continue it.
        let mut fields: Vec<(&str, String, Vec<&str>)> = Vec::new();
        for line in block.lines() {
            let key = line.split_once(':').filter(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            });
            match key {
                Some((key, value)) => fields.push((key, value.trim().to_string(), Vec::new())),
                None => {
                    if let Some((_, _, rest)) = fields.last_mut() {
                        rest.push(line);
                    }
                }
            }
            let item = line.trim_start().trim_start_matches('-').trim_start();
            if let Some(cond) = item
                .strip_prefix("@if(")
                .or_else(|| item.strip_prefix("@elif("))
                && let Some((cond, note)) = cond.split_once("):")
            {
                let note = (cond.trim().to_string(), note.trim().to_string());
                doc.flag_notes.push(note);
            }
        }
        for (key, value, rest) in fields {
            let lines = yaml_lines(&value, &rest);
            match key {
                "description" => doc.description = lines.join("\n"),
                "contributors" | "contributor" | "author" => doc.contributors.extend(lines),
                "use" => doc.uses = lines,
                "license" => doc.license = lines,
                "options" => {
                    for line in lines {
                        if let Some((name, note)) = line.split_once(':') {
                            let note = (name.trim().to_string(), note.trim().to_string());
                            doc.flag_notes.push(note);
                        }
                    }
                }
                _ => {}
            }
        }
        doc
    }
}

/// A YAML scalar or list as lines of text: `value` is what followed the
/// key's colon, `rest` the field's indented continuation lines. A list
/// gives one line per item, a `|` block one line per line; other scalars
/// are folded into one line.
fn yaml_lines(value: &str, rest: &[&str]) -> Vec<String> {
    let indent = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let body: Vec<&str> = rest
        .iter()
        .map(|l| l.get(indent..).unwrap_or("").trim_end())
        .collect();
    if value.starts_with('|') {
        let text = body.join("\n");
        return text.trim().lines().map(str::to_string).collect();
    }
    if value.is_empty() && body.iter().any(|l| l.starts_with("- ")) {
        // A list; an item's continuation lines are indented under it.
        let mut items: Vec<String> = Vec::new();
        for line in body {
            if let Some(item) = line.strip_prefix("- ") {
                items.push(item.trim().to_string());
            } else if let Some(last) = items.last_mut()
                && !line.trim().is_empty()
            {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
        return items;
    }
    // A (possibly quoted) scalar folded over several lines.
    let mut joined = value.to_string();
    for line in body {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if joined.ends_with('\\') {
            // An escaped line break in a double-quoted scalar: no space.
            joined.pop();
        } else if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(line);
    }
    let text = if let Some(quoted) = joined.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        unescape_double_quoted(quoted)
    } else if let Some(quoted) = joined.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        quoted.replace("''", "'")
    } else {
        joined
    };
    let text = text.trim();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_string()]
    }
}

/// The escapes LYGIA's double-quoted YAML scalars use.
fn unescape_double_quoted(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// A module-scope declaration: what the LYGIA window lists and imports.
struct Declaration {
    /// `ItemKind` variant name.
    kind: &'static str,
    name: String,
    /// The declaration up to its body (`fn f(x: f32) -> f32`), on one line.
    signature: String,
    /// The `//` comment lines right above it.
    doc: String,
}

/// The `fn`s, `const`s and `struct`s declared at column 0 of `text` (which
/// is where LYGIA declares all of them). A function declared once per `@if`
/// branch is listed once per distinct signature, a constant once.
fn declarations(text: &str) -> Vec<Declaration> {
    let blanked = wesl_scan::blank_comments(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut offset = 0;
    let mut out: Vec<Declaration> = Vec::new();
    for (row, line) in blanked.lines().enumerate() {
        let start = offset;
        offset += line.len() + 1;
        let (kind, end) = if line.starts_with("fn ") {
            ("Fn", '{')
        } else if line.starts_with("const ") {
            ("Const", ';')
        } else if line.starts_with("struct ") {
            ("Struct", '{')
        } else {
            continue;
        };
        let rest = &blanked[start..];
        let Some(stop) = rest.find(end) else { continue };
        let mut signature = rest[..stop]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if signature.chars().count() > 120 {
            signature = signature.chars().take(117).collect::<String>() + "…";
        }
        let name: String = signature
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let doc_lines: Vec<&str> = lines[..row]
            .iter()
            .rev()
            .skip_while(|l| l.starts_with('@'))
            .map_while(|l| l.trim_start().strip_prefix("//"))
            .collect();
        let doc = doc_lines
            .iter()
            .rev()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let duplicate = out
            .iter()
            .any(|d| d.name == name && (kind == "Const" || d.signature == signature));
        if !name.is_empty() && !duplicate {
            out.push(Declaration {
                kind,
                name,
                signature,
                doc,
            });
        }
    }
    out
}

/// Directory entries sorted by path, so the generated code is stable.
fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    entries
}
