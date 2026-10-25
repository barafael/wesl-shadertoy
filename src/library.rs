//! The LYGIA window: a searchable index of the bundled LYGIA modules.
//!
//! `build.rs` reads every module once at build time into [`MODULES`]: the
//! doc block at the top of the file (description, contributors, the `use:`
//! lines, license, notes on its `@if` flags) and the declarations the file
//! actually makes. The `use:` lines are often stale GLSL-style signatures,
//! so the window shows the declarations as written in WGSL, with the doc
//! text around them. Each one has an "import" button that adds
//! `import lygia::module::name;` to the editor text (see [`add_import`]),
//! and a module's source opens read-only next to the list.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::editor::{self, HighlightCache};
use crate::wesl_scan;

/// One bundled LYGIA module, as indexed by `build.rs`.
pub struct ModuleDoc {
    /// Below the package root, `::`-separated: `generative::snoise`.
    pub path: &'static str,
    pub description: &'static str,
    pub contributors: &'static [&'static str],
    /// The doc block's `use:` lines (often GLSL-flavoured).
    pub uses: &'static [&'static str],
    pub license: &'static [&'static str],
    /// `(condition, what it does)` for its feature flags, from the doc
    /// block: `("CENTER_2D", "Uses custom 2D center point …")`.
    pub flag_notes: &'static [(&'static str, &'static str)],
    pub items: &'static [Item],
    /// Feature flags its `@if` / `@elif` attributes test.
    pub flags: &'static [&'static str],
    /// Names it reads from the `constants::` virtual module.
    pub constants: &'static [&'static str],
    /// The LYGIA modules it imports (paths like [`Self::path`]).
    pub imports: &'static [&'static str],
}

/// A module-scope declaration of a [`ModuleDoc`].
pub struct Item {
    pub kind: ItemKind,
    pub name: &'static str,
    /// Up to the body, on one line: `fn snoise2(v: vec2f) -> f32`.
    pub signature: &'static str,
    /// The `//` comment right above it.
    pub doc: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Fn,
    Const,
    Struct,
}

include!(concat!(env!("OUT_DIR"), "/lygia_docs.rs"));

impl ModuleDoc {
    /// Its top-level directory: `generative` for `generative::snoise`.
    pub fn category(&self) -> &'static str {
        self.path.split("::").next().unwrap_or(self.path)
    }
}

/// The module at `path` (`generative::snoise`).
pub fn module(path: &str) -> Option<&'static ModuleDoc> {
    MODULES
        .binary_search_by(|m| m.path.cmp(path))
        .ok()
        .map(|i| &MODULES[i])
}

/// The top-level directories, in order.
fn categories() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = MODULES.iter().map(|m| m.category()).collect();
    out.dedup();
    out
}

/// The LYGIA window's state.
#[derive(Resource, Default)]
pub struct Library {
    query: String,
    category: Option<&'static str>,
    /// The module whose source is open next to the list.
    source: Option<&'static ModuleDoc>,
    /// What the last import button did.
    note: Option<String>,
    /// Lowercased search text per module (path, item names, description),
    /// built on first use.
    haystacks: Vec<String>,
    /// The source view's own highlight cache, so it does not take turns
    /// with the WGSL window over the shared one.
    highlight: HighlightCache,
}

impl Library {
    /// The modules that pass the category filter and contain every word of
    /// the query (in their path, item names or description).
    fn filtered(&mut self) -> Vec<&'static ModuleDoc> {
        if self.haystacks.is_empty() {
            self.haystacks = MODULES
                .iter()
                .map(|m| {
                    let names: Vec<&str> = m.items.iter().map(|i| i.name).collect();
                    format!("{} {} {}", m.path, names.join(" "), m.description).to_lowercase()
                })
                .collect();
        }
        let query = self.query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        MODULES
            .iter()
            .zip(&self.haystacks)
            .filter(|(m, hay)| {
                self.category.is_none_or(|c| m.category() == c)
                    && words.iter().all(|w| hay.contains(w))
            })
            .map(|(m, _)| m)
            .collect()
    }
}

/// The window's contents. Import buttons edit `text`, the editor document,
/// in place (like the Constants window's write-back), so autosave and the
/// editor's undo see an ordinary edit.
pub fn show(ui: &mut egui::Ui, lib: &mut Library, text: &mut String) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut lib.query)
                .hint_text("search modules, functions, descriptions")
                .desired_width(260.0),
        );
        if ui
            .add_enabled(
                !lib.query.is_empty() || lib.category.is_some(),
                egui::Button::new("clear"),
            )
            .clicked()
        {
            lib.query.clear();
            lib.category = None;
        }
    });
    ui.horizontal_wrapped(|ui| {
        for category in categories() {
            let on = lib.category == Some(category);
            if ui.selectable_label(on, category).clicked() {
                lib.category = (!on).then_some(category);
            }
        }
    });
    let shown = lib.filtered();
    ui.horizontal(|ui| {
        ui.weak(format!("{} of {} modules", shown.len(), MODULES.len()));
        if let Some(note) = &lib.note {
            ui.label(egui::RichText::new(note).color(crate::theme::OK_COLOR));
        }
    });
    ui.separator();

    let Some(source) = lib.source else {
        module_list(ui, lib, &shown, text);
        return;
    };
    ui.columns(2, |cols| {
        module_list(&mut cols[0], lib, &shown, text);
        let ui = &mut cols[1];
        ui.horizontal(|ui| {
            ui.monospace(format!("lygia::{}", source.path));
            ui.weak("(read-only)");
            if ui.button("close").clicked() {
                lib.source = None;
            }
        });
        let code = crate::compile::lygia_source(source.path).unwrap_or_default();
        editor::show_code_view(ui, code, &mut lib.highlight);
    });
}

/// The scrolling list of modules, each a collapsible entry.
fn module_list(
    ui: &mut egui::Ui,
    lib: &mut Library,
    shown: &[&'static ModuleDoc],
    text: &mut String,
) {
    egui::ScrollArea::vertical()
        .id_salt("lygia_modules")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if shown.is_empty() {
                ui.weak("No module matches.");
            }
            // A short result list opens up by itself.
            let open = shown.len() <= 3;
            for &m in shown {
                egui::CollapsingHeader::new(egui::RichText::new(m.path).monospace())
                    .id_salt(m.path)
                    .default_open(open)
                    .show(ui, |ui| module_entry(ui, lib, m, text));
            }
        });
}

/// One expanded module: what it does, its declarations with import
/// buttons, and a way to read its source.
fn module_entry(ui: &mut egui::Ui, lib: &mut Library, m: &'static ModuleDoc, text: &mut String) {
    if !m.description.is_empty() {
        ui.label(m.description);
    }
    if !m.contributors.is_empty() {
        ui.label(
            egui::RichText::new(format!("by {}", m.contributors.join(", ")))
                .small()
                .weak(),
        );
    }
    if !m.flags.is_empty() {
        let flags = ui.label(
            egui::RichText::new(format!("flags: {}", m.flags.join(", ")))
                .small()
                .weak(),
        );
        if !m.flag_notes.is_empty() {
            flags.on_hover_ui(|ui| {
                for (cond, note) in m.flag_notes {
                    ui.label(format!("{cond}: {note}"));
                }
            });
        }
    }
    if !m.constants.is_empty() {
        ui.label(
            egui::RichText::new(format!("reads constants::{}", m.constants.join(", ")))
                .small()
                .weak(),
        );
    }
    // Functions first: they are what most imports are after.
    let kinds = [ItemKind::Fn, ItemKind::Struct, ItemKind::Const];
    let items = kinds
        .iter()
        .flat_map(|&kind| m.items.iter().filter(move |i| i.kind == kind));
    for item in items {
        ui.horizontal(|ui| {
            let import = ui
                .small_button("import")
                .on_hover_text(format!("import lygia::{}::{};", m.path, item.name));
            if import.clicked() {
                lib.note = Some(match add_import(text, m.path, item.name) {
                    Added::Already => format!("{} is already imported", item.name),
                    Added::Merged | Added::Inserted => format!("imported {}", item.name),
                });
            }
            let signature = ui.add(
                egui::Label::new(egui::RichText::new(item.signature).monospace())
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            if !item.doc.is_empty() {
                signature.on_hover_text(item.doc);
            }
        });
    }
    ui.horizontal(|ui| {
        let open = lib.source.is_some_and(|s| std::ptr::eq(s, m));
        if ui.selectable_label(open, "source").clicked() {
            lib.source = (!open).then_some(m);
        }
        if !m.uses.is_empty() {
            ui.weak("use:").on_hover_text(m.uses.join("\n"));
        }
        if !m.license.is_empty() {
            ui.weak("license").on_hover_text(m.license.join("\n"));
        }
    });
    if !m.imports.is_empty() {
        // The modules it builds on, each a link to its source.
        ui.horizontal_wrapped(|ui| {
            ui.weak("imports");
            for &path in m.imports {
                if ui.small_button(path).clicked() {
                    lib.source = module(path);
                }
            }
        });
    }
}

/// What [`add_import`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// The item was imported already; nothing changed.
    Already,
    /// Added to an existing import of the same module.
    Merged,
    /// A new import line.
    Inserted,
}

/// Import `item` of the LYGIA module `module` (`generative::snoise`) into
/// `text`, unless it is imported already. Joins a one-line import of the
/// same module (`import lygia::m::a;` or `import lygia::m::{a, b};`) if
/// there is one, else adds a line after the last import (or, without any,
/// after the comments the text starts with).
pub fn add_import(text: &mut String, module: &str, item: &str) -> Added {
    let wanted: Vec<&str> = module.split("::").chain([item]).collect();
    if wesl_scan::package_paths(text, "lygia")
        .iter()
        .any(|path| path.iter().map(String::as_str).eq(wanted.iter().copied()))
    {
        return Added::Already;
    }

    // Offsets in the comment-free text are offsets in `text` too.
    let blanked = wesl_scan::blank_comments(text);
    let prefix = format!("import lygia::{module}::");
    let mut offset = 0;
    let mut after_imports = None;
    for line in blanked.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let code = line.trim();
        if !code.starts_with("import ") {
            continue;
        }
        // The statement may continue over the next lines.
        let end = blanked[start..]
            .find(';')
            .map_or(blanked.len(), |i| start + i);
        after_imports = Some(
            blanked[end..]
                .find('\n')
                .map_or(blanked.len(), |i| end + i + 1),
        );
        let Some(rest) = code.strip_prefix(&prefix).and_then(|r| r.strip_suffix(';')) else {
            continue;
        };
        let at = start + (line.len() - line.trim_start().len()) + prefix.len();
        let rest = rest.trim();
        if let Some(list) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            let close = at + text[at..].find('}').expect("the import's closing brace");
            let sep = if list.trim().is_empty() { "" } else { ", " };
            text.insert_str(close, &format!("{sep}{item}"));
            return Added::Merged;
        }
        if rest.chars().all(|c| c.is_alphanumeric() || c == '_') {
            text.replace_range(at..at + rest.len(), &format!("{{{rest}, {item}}}"));
            return Added::Merged;
        }
    }

    let line = format!("import lygia::{module}::{item};\n");
    let at = after_imports.unwrap_or_else(|| {
        // After the leading comments and blank lines (a header, the
        // `// wesl-flags:` pragmas), at the start of the code.
        let mut at = 0;
        for l in text.split_inclusive('\n') {
            let t = l.trim();
            if !(t.is_empty() || t.starts_with("//")) {
                break;
            }
            at += l.len();
        }
        at
    });
    let separate = after_imports.is_none() && !text[at..].is_empty();
    let mut insert = line;
    if separate {
        insert.push('\n');
    }
    if at > 0 && !text[..at].ends_with('\n') {
        insert.insert(0, '\n');
    }
    text.insert_str(at, &insert);
    Added::Inserted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_reads_doc_blocks_and_declarations() {
        assert!(MODULES.len() > 300, "{}", MODULES.len());
        let snoise = module("generative::snoise").expect("snoise indexed");
        assert_eq!(snoise.category(), "generative");
        assert!(!snoise.description.is_empty());
        assert!(
            snoise
                .items
                .iter()
                .any(|i| i.kind == ItemKind::Fn && i.signature.starts_with("fn snoise2(")),
            "{:?}",
            snoise.items.iter().map(|i| i.signature).collect::<Vec<_>>()
        );
        // Its own imports, resolved to modules.
        assert!(
            snoise.imports.contains(&"math::permute"),
            "{:?}",
            snoise.imports
        );

        let marble = module("distort::marble").unwrap();
        assert_eq!(marble.flags, ["MARBLE_PERIOD"]);
        assert_eq!(marble.constants, ["MARBLE_PERIOD"]);
        assert!(marble.flag_notes.iter().any(|(c, _)| *c == "MARBLE_PERIOD"));
        let reset = marble
            .items
            .iter()
            .find(|i| i.name == "marbleReset")
            .unwrap();
        assert!(
            reset.doc.starts_with("First frame of a cycle"),
            "{}",
            reset.doc
        );
        // `PERIOD` is declared in both @if branches but listed once.
        assert_eq!(
            marble.items.iter().filter(|i| i.name == "PERIOD").count(),
            1
        );

        // A double-quoted description folded over several lines.
        let ratio = module("space::ratio").unwrap();
        assert!(
            ratio.description.contains("in a similar way"),
            "{}",
            ratio.description
        );
        assert!(!ratio.description.contains('\\'), "{}", ratio.description);

        // Constants, `@elif` notes, and `filter` (a WGSL reserved word) as a path.
        let consts = module("math::consts").unwrap();
        assert!(
            consts
                .items
                .iter()
                .any(|i| i.kind == ItemKind::Const && i.name == "PI")
        );
        let hsv2ryb = module("color::space::hsv2ryb").unwrap();
        assert!(hsv2ryb.flag_notes.iter().any(|(c, _)| *c == "RYB_FAST"));
        assert!(MODULES.iter().any(|m| m.category() == "filter"));
        // Every module's source is reachable for the source view.
        for m in MODULES {
            assert!(crate::compile::lygia_source(m.path).is_some(), "{}", m.path);
        }
    }

    #[test]
    fn search_matches_paths_functions_and_descriptions() {
        let mut lib = Library::default();
        let paths =
            |lib: &mut Library| -> Vec<&str> { lib.filtered().iter().map(|m| m.path).collect() };
        assert_eq!(paths(&mut lib).len(), MODULES.len());
        lib.query = "snoise".into();
        assert!(paths(&mut lib).contains(&"generative::snoise"));
        // A function name only.
        lib.query = "marbleBake1".into();
        assert_eq!(paths(&mut lib), ["distort::marble"]);
        // Every word must match, in any field.
        lib.query = "MARBLING texture".into();
        assert_eq!(paths(&mut lib), ["distort::marble"]);
        lib.query.clear();
        lib.category = Some("sdf");
        let sdf = paths(&mut lib);
        assert!(!sdf.is_empty() && sdf.iter().all(|p| p.starts_with("sdf::")));
    }

    #[test]
    fn imports_are_inserted_after_the_existing_ones() {
        let mut text = String::from(
            "// header\n\
             import package::shadertoy::i_time;\n\
             import lygia::color::palette::spectral::spectral;\n\
             \n\
             fn main_image(p: vec2f) -> vec4f { return vec4f(0.0); }\n",
        );
        assert_eq!(
            add_import(&mut text, "generative::snoise", "snoise2"),
            Added::Inserted
        );
        assert!(
            text.contains("spectral::spectral;\nimport lygia::generative::snoise::snoise2;\n\nfn"),
            "{text}"
        );
        // Again: nothing to do.
        let before = text.clone();
        assert_eq!(
            add_import(&mut text, "generative::snoise", "snoise2"),
            Added::Already
        );
        assert_eq!(text, before);
        // Same module: merged into the line.
        assert_eq!(
            add_import(&mut text, "generative::snoise", "snoise3"),
            Added::Merged
        );
        assert!(
            text.contains("import lygia::generative::snoise::{snoise2, snoise3};"),
            "{text}"
        );
        assert_eq!(
            add_import(&mut text, "generative::snoise", "snoise4"),
            Added::Merged
        );
        assert!(
            text.contains("import lygia::generative::snoise::{snoise2, snoise3, snoise4};"),
            "{text}"
        );
        // A tree import covering it counts as imported.
        let mut tree = String::from("import lygia::color::{blend::darken::blendDarken};\n");
        assert_eq!(
            add_import(&mut tree, "color::blend::darken", "blendDarken"),
            Added::Already
        );
        // Commented-out imports do not.
        let mut commented = String::from("// import lygia::math::consts::PI;\nfn f() {}\n");
        assert_eq!(
            add_import(&mut commented, "math::consts", "PI"),
            Added::Inserted
        );
        assert_eq!(
            commented,
            "// import lygia::math::consts::PI;\nimport lygia::math::consts::PI;\n\nfn f() {}\n"
        );
    }

    #[test]
    fn imports_follow_multi_line_imports() {
        let mut text =
            String::from("import lygia::color::space::{\n    rgb2hsv::rgb2hsv,\n};\nfn f() {}\n");
        add_import(&mut text, "math::consts", "PI");
        assert_eq!(
            text,
            "import lygia::color::space::{\n    rgb2hsv::rgb2hsv,\n};\nimport lygia::math::consts::PI;\nfn f() {}\n"
        );
        let mut empty = String::new();
        add_import(&mut empty, "math::consts", "PI");
        assert_eq!(empty, "import lygia::math::consts::PI;\n");
    }

    /// Search, expand, click "import": the editor text gains the import.
    #[test]
    fn import_button_edits_the_document() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut lib = Library {
            query: "marbleBake1".into(),
            ..Default::default()
        };
        let mut text = String::from("fn main_image(p: vec2f) -> vec4f { return vec4f(0.0); }\n");
        let run = |lib: &mut Library, text: &mut String, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| show(ui, lib, text));
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            shapes
        };
        // The only match opens by itself; find its first "import" button.
        let mut shapes = run(&mut lib, &mut text, vec![]);
        for _ in 0..2 {
            shapes = run(&mut lib, &mut text, vec![]);
        }
        let button = shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text() == "import" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("an import button");
        let click = |pressed| egui::Event::PointerButton {
            pos: button,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run(&mut lib, &mut text, vec![egui::Event::PointerMoved(button)]);
        run(&mut lib, &mut text, vec![click(true)]);
        run(&mut lib, &mut text, vec![click(false)]);
        // Functions come first, so the first button is marbleEncode's.
        assert!(
            text.starts_with("import lygia::distort::marble::marbleEncode;\n\nfn main_image"),
            "{text}"
        );
        assert!(lib.note.is_some());
    }
}
