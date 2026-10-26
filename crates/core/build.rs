//! Embeds the shader sources the compiler works with:
//!
//! * `examples/*.wesl` (at the repository root) → `$OUT_DIR/examples.rs`,
//!   the table behind `crate::examples::ALL`. Each file's leading comment
//!   block carries its metadata (see `crate::examples` for the format); it is
//!   parsed here, so a malformed header fails the build instead of showing
//!   up at runtime.
//! * `vendor/lygia/**/*.wesl` → `$OUT_DIR/lygia.rs`, the `lygia` WESL package
//!   as a `wesl::package::StaticPackage` tree (the same shape
//!   `wesl::PackageBuilder` generates, without building `wesl` for the host).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo::rerun-if-changed=build.rs");

    let examples = root.join("examples");
    println!("cargo::rerun-if-changed={}", examples.display());
    std::fs::write(out.join("examples.rs"), examples_table(&examples)).unwrap();

    let lygia = root.join("vendor/lygia");
    println!("cargo::rerun-if-changed={}", lygia.display());
    std::fs::write(out.join("lygia.rs"), lygia_package(&lygia)).unwrap();
}

/// `ALL`: one `Example` literal per file, sorted by display name.
fn examples_table(dir: &Path) -> String {
    let mut examples: Vec<(String, String)> = sorted_entries(dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "wesl"))
        .map(|path| {
            println!("cargo::rerun-if-changed={}", path.display());
            let text = std::fs::read_to_string(&path).unwrap();
            let header = Header::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (header.name.clone(), header.literal(&path, &text))
        })
        .collect();
    examples.sort_by_key(|(name, _)| name.to_lowercase());
    let mut out = String::from("pub static ALL: &[Example] = &[\n");
    for (_, literal) in examples {
        out.push_str(&literal);
    }
    out.push_str("];\n");
    out
}

/// An example's metadata, from the comment block it starts with:
///
/// ```text
/// // Name — one-line summary, which may continue
/// // on the next comment lines.
/// // author: …
/// // license: …
/// // source: https://…        (optional)
/// // tags: 2d, noise          (optional)
/// // teaches: what a reader learns from it (optional, shown on its card)
/// ```
struct Header {
    name: String,
    summary: String,
    author: String,
    license: String,
    source: String,
    tags: Vec<String>,
    teaches: String,
}

impl Header {
    fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().map_while(|l| l.strip_prefix("//"));
        let first = lines.next().ok_or("must start with `// Name — summary`")?;
        let (name, summary) = first
            .split_once('—')
            .ok_or("the first line must read `// Name — summary`")?;
        let mut header = Header {
            name: name.trim().to_string(),
            summary: summary.trim().to_string(),
            author: String::new(),
            license: String::new(),
            source: String::new(),
            tags: Vec::new(),
            teaches: String::new(),
        };
        let mut in_summary = true;
        for line in lines {
            let line = line.trim();
            let field = line
                .split_once(':')
                .filter(|(k, _)| ["author", "license", "source", "tags", "teaches"].contains(k));
            match field {
                Some((key, value)) => {
                    in_summary = false;
                    let value = value.trim().to_string();
                    match key {
                        "author" => header.author = value,
                        "license" => header.license = value,
                        "source" => header.source = value,
                        "teaches" => header.teaches = value,
                        _ => header.tags = value.split(',').map(|t| t.trim().to_string()).collect(),
                    }
                }
                None if in_summary && !line.is_empty() => {
                    header.summary.push(' ');
                    header.summary.push_str(line);
                }
                None => in_summary = false,
            }
        }
        if header.license.is_empty() {
            return Err("missing `// license: …`".into());
        }
        if header.author.is_empty() && header.source.is_empty() {
            return Err("needs `// author: …` or `// source: …`".into());
        }
        Ok(header)
    }

    fn literal(&self, path: &Path, text: &str) -> String {
        // Tags the code itself implies, so they cannot go stale.
        let mut tags = self.tags.clone();
        let mut imply = |tag: &str, cond: bool| {
            if cond && !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
        };
        imply("buffers", text.contains("fn buffer_a"));
        imply("mouse", text.contains("i_mouse"));
        imply("lygia", text.contains("lygia::"));
        let tags = tags
            .iter()
            .map(|t| format!("{t:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "    Example {{\n        name: {:?},\n        summary: {:?},\n        author: {:?},\n        \
             license: {:?},\n        source: {:?},\n        tags: &[{tags}],\n        \
             teaches: {:?},\n        code: include_str!({:?}),\n    }},\n",
            self.name,
            self.summary,
            self.author,
            self.license,
            self.source,
            self.teaches,
            path.display().to_string(),
        )
    }
}

/// `LYGIA`: the package root and one `StaticPackageModule` per directory
/// and `.wesl` file. A file and a directory of the same name (`quat.wesl`
/// next to `quat/`) are one module with both a source and submodules.
fn lygia_package(dir: &Path) -> String {
    let mut out = String::new();
    let root = module(&mut out, &mut 0, "lygia", None, Some(dir));
    let _ = writeln!(
        out,
        "pub static LYGIA: StaticPackage = StaticPackage {{\n    crate_name: \"lygia\",\n    \
         root: &{root},\n    dependencies: &[],\n}};"
    );
    out
}

/// Emit the module `name` (source file `file`, children in `dir`) and its
/// descendants into `out`; returns the static's identifier.
fn module(
    out: &mut String,
    next_id: &mut usize,
    name: &str,
    file: Option<&Path>,
    dir: Option<&Path>,
) -> String {
    let mut children = Vec::new();
    if let Some(dir) = dir {
        let entries = sorted_entries(dir);
        let mut names: Vec<String> = entries
            .iter()
            .filter(|p| p.is_dir() || p.extension().is_some_and(|e| e == "wesl"))
            .filter_map(|p| Some(p.file_stem()?.to_str()?.to_string()))
            .collect();
        // A file and a directory of the same stem need not be adjacent in
        // path order (`quat`, `quat-x`, `quat.wesl`), so sort the stems.
        names.sort();
        names.dedup();
        for child in names {
            let child_file = dir.join(format!("{child}.wesl"));
            let child_dir = dir.join(&child);
            children.push(module(
                out,
                next_id,
                &child,
                child_file.is_file().then_some(child_file.as_path()),
                child_dir.is_dir().then_some(child_dir.as_path()),
            ));
        }
    }
    let id = format!("M{}", *next_id);
    *next_id += 1;
    let source = match file {
        Some(file) => format!("include_str!({:?})", file.display().to_string()),
        None => "\"\"".to_string(),
    };
    let children = children
        .iter()
        .map(|c| format!("&{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "static {id}: StaticPackageModule = StaticPackageModule {{ name: {name:?}, source: \
         {source}, submodules: &[{children}] }};"
    );
    id
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
