//! WESL feature flags and `constants::` values, kept in the shader itself.
//!
//! WESL compiles code under `@if(FLAG)` only while `FLAG` is on, and reads
//! numeric settings from the virtual module `constants` (`import
//! constants::MARBLE_PERIOD;`). LYGIA uses both: `CENTER_2D` moves the
//! origin of its SDFs, `MIXOKLAB_SRGB` switches a colour mix to sRGB, and
//! `@if(MARBLE_PERIOD)` reads its period from `constants::MARBLE_PERIOD`.
//! User code may test flags of its own as well.
//!
//! The settings live in the document as comment lines, so a saved, copied
//! or shared shader compiles the same way anywhere:
//!
//! ```wgsl
//! // wesl-flags: CENTER_2D, MIXOKLAB_SRGB
//! // wesl-const: MARBLE_PERIOD = 90.0
//! ```
//!
//! [`Pragmas`] (in the library) reads them for the compile ([`crate::compile`]): the flags
//! become WESL features, the constants the source of the `constants`
//! module. A constant also turns on its same-named flag, the pairing LYGIA
//! relies on (`@if(MARBLE_PERIOD)` guards the `import
//! constants::MARBLE_PERIOD;`). Flags change which code exists, so unlike
//! the Constants window's tweaks they cannot apply live: the Flags window
//! ([`show`]) rewrites the lines and asks for a recompile.

use bevy::prelude::Resource;
use bevy_egui::egui;

use wesl_shadertoy_core::pragmas::Pragmas;

use crate::library;
use crate::wesl_scan;

/// Values LYGIA's constants have while their flag is off, as starting
/// values for the Flags window. Others start at `0.0`.
const DEFAULTS: &[(&str, &str)] = &[
    ("CENTER_2D", "vec2f(0.5)"),
    ("CENTER_3D", "vec3f(0.5)"),
    ("CENTER_4D", "vec4f(0.5)"),
    ("MARBLE_PERIOD", "60.0"),
    ("BRACKETING_ANGLE_DELTA", "0.15707963"),
    ("CAMERA_NEAR_CLIP", "0.1"),
    ("CAMERA_FAR_CLIP", "100.0"),
    (
        "CAMERA_PROJECTION_MATRIX",
        "mat4x4f(1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.)",
    ),
    (
        "INVERSE_CAMERA_PROJECTION_MATRIX",
        "mat4x4f(1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.)",
    ),
];

/// A flag or constant the shader can set, and who reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub name: String,
    /// Read from `constants::` (so it needs a value), not just tested.
    pub constant: bool,
    /// Where: LYGIA module paths, or `your code`.
    pub users: Vec<&'static str>,
    /// What the modules' docs say it does: `"!YUV_SDTV: HDTV (default)"`.
    pub notes: Vec<String>,
}

/// How [`Usage::users`] names the editor text itself.
const USER_CODE: &str = "your code";

/// The flags and constants that `text` and the LYGIA modules it imports,
/// directly or through other modules, test or read. Sorted by name.
pub fn usages(text: &str) -> Vec<Usage> {
    let mut out: Vec<Usage> = Vec::new();
    let mut note = |name: &str, constant: bool, user: &'static str, notes: &[String]| {
        let usage = match out.iter_mut().position(|u| u.name == name) {
            Some(i) => &mut out[i],
            None => {
                out.push(Usage {
                    name: name.to_string(),
                    constant: false,
                    users: Vec::new(),
                    notes: Vec::new(),
                });
                out.last_mut().expect("just pushed")
            }
        };
        usage.constant |= constant;
        if !usage.users.contains(&user) {
            usage.users.push(user);
        }
        for n in notes {
            if !usage.notes.contains(n) {
                usage.notes.push(n.clone());
            }
        }
    };
    for flag in wesl_scan::flag_names(text) {
        note(&flag, false, USER_CODE, &[]);
    }
    for name in wesl_scan::constant_names(text) {
        note(&name, true, USER_CODE, &[]);
    }
    for m in imported_modules(text) {
        let notes_for = |name: &str| -> Vec<String> {
            m.flag_notes
                .iter()
                .filter(|(cond, _)| mentions(cond, name))
                .map(|(cond, text)| format!("{cond}: {text}"))
                .collect()
        };
        for flag in m.flags {
            note(flag, false, m.path, &notes_for(flag));
        }
        for name in m.constants {
            note(name, true, m.path, &notes_for(name));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Whether the condition `cond` (`CAMERA_NEAR_CLIP && !X`) names `name`.
fn mentions(cond: &str, name: &str) -> bool {
    cond.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w == name)
}

/// The LYGIA modules `text` imports, and the ones those import, in
/// breadth-first order.
fn imported_modules(text: &str) -> Vec<&'static library::ModuleDoc> {
    let mut out: Vec<&'static library::ModuleDoc> = Vec::new();
    for path in wesl_scan::package_paths(text, "lygia") {
        // The longest prefix that is a module (the rest names an item).
        let found = (1..=path.len())
            .rev()
            .find_map(|n| library::module(&path[..n].join("::")));
        if let Some(m) = found
            && !out.iter().any(|o| std::ptr::eq(*o, m))
        {
            out.push(m);
        }
    }
    let mut i = 0;
    while i < out.len() {
        for import in out[i].imports {
            if let Some(m) = library::module(import)
                && !out.iter().any(|o| std::ptr::eq(*o, m))
            {
                out.push(m);
            }
        }
        i += 1;
    }
    out
}

/// The Flags window's state.
#[derive(Resource, Default)]
pub struct FlagsPanel {
    /// [`usages`] of the text it was computed for.
    cache: Option<(String, Vec<Usage>)>,
    /// The constant fields' text, while being edited.
    edits: Vec<(String, String)>,
}

/// The Flags window's contents: a checkbox per flag, a value per constant.
/// Changes rewrite the pragma lines of `text`; returns whether it did, so
/// the caller recompiles.
pub fn show(ui: &mut egui::Ui, panel: &mut FlagsPanel, text: &mut String) -> bool {
    if panel.cache.as_ref().is_none_or(|(t, _)| t != text) {
        panel.cache = Some((text.clone(), usages(text)));
    }
    let mut rows = panel
        .cache
        .as_ref()
        .map(|(_, u)| u.clone())
        .unwrap_or_default();
    let before = Pragmas::parse(text);
    // Set in the code but no longer used: still listed, to turn off.
    for name in before
        .flags
        .iter()
        .chain(before.constants.iter().map(|(n, _)| n))
    {
        if !rows.iter().any(|u| &u.name == name) {
            rows.push(Usage {
                name: name.clone(),
                constant: before.constant(name).is_some(),
                users: Vec::new(),
                notes: Vec::new(),
            });
        }
    }
    let mut pragmas = before.clone();

    if rows.is_empty() {
        ui.label(
            "Neither this shader nor the LYGIA modules it imports test any \
             feature flags. Flags show up here for imports like",
        );
        ui.code("import lygia::sdf::circleSDF::circleSDF;");
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("flags_grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for usage in &rows {
                        row(ui, panel, &mut pragmas, usage);
                        ui.end_row();
                    }
                });
        });
    }
    ui.separator();
    ui.weak(
        "Flags select code, so changes recompile. They are kept in the code \
         as `// wesl-flags:` and `// wesl-const:` lines; a constant also \
         turns on its same-named flag.",
    );

    if pragmas == before {
        return false;
    }
    pragmas.write(text);
    true
}

/// One flag or constant: its checkbox, value field and users.
fn row(ui: &mut egui::Ui, panel: &mut FlagsPanel, pragmas: &mut Pragmas, usage: &Usage) {
    let name = usage.name.as_str();
    let mut on = pragmas.enabled(name);
    let checkbox = ui.checkbox(&mut on, egui::RichText::new(name).monospace());
    let checkbox = if usage.notes.is_empty() {
        checkbox
    } else {
        checkbox.on_hover_text(usage.notes.join("\n"))
    };
    let default = DEFAULTS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("0.0", |(_, v)| v);
    if checkbox.changed() {
        pragmas.set_flag(name, on);
        if on && usage.constant {
            let value = edit_buffer(panel, name).map_or(default.to_string(), |b| b.clone());
            pragmas.set_constant(name, &value);
        }
    }

    if usage.constant {
        let current = pragmas.constant(name).unwrap_or(default).to_string();
        let i = match panel.edits.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                panel.edits.push((name.to_string(), current.clone()));
                panel.edits.len() - 1
            }
        };
        let buffer = &mut panel.edits[i].1;
        let field = ui.add(
            egui::TextEdit::singleline(buffer)
                .id_salt(("flag_const", name))
                .font(egui::TextStyle::Monospace)
                .desired_width(160.0),
        );
        if field.lost_focus() {
            // Committing a value sets the constant (and so its flag).
            let value = buffer.trim().to_string();
            if !value.is_empty() && (pragmas.constant(name) != Some(value.as_str())) {
                pragmas.set_constant(name, &value);
            }
        } else if !field.has_focus() {
            *buffer = current;
        }
    } else {
        ui.label("");
    }

    let users = if usage.users.is_empty() {
        "not used".to_string()
    } else {
        usage.users.join(", ")
    };
    ui.label(egui::RichText::new(users).small().weak());
}

/// The text in constant `name`'s value field, if it has one.
fn edit_buffer<'a>(panel: &'a FlagsPanel, name: &str) -> Option<&'a String> {
    panel.edits.iter().find(|(n, _)| n == name).map(|(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragmas_round_trip() {
        let text = "// Title\n\
                    // wesl-flags: CENTER_2D, MIXOKLAB_SRGB\n\
                    // wesl-const: MARBLE_PERIOD = 90.0\n\
                    import lygia::distort::marble::marbleBake1;\n\
                    // wesl-const: CENTER_2D = vec2f(0.25, 0.75)\n";
        let p = Pragmas::parse(text);
        assert_eq!(p.flags, ["CENTER_2D", "MIXOKLAB_SRGB"]);
        assert_eq!(
            p.constants,
            [
                ("MARBLE_PERIOD".to_string(), "90.0".to_string()),
                ("CENTER_2D".to_string(), "vec2f(0.25, 0.75)".to_string()),
            ]
        );
        // A constant turns its flag on.
        assert!(p.enabled("MARBLE_PERIOD"));
        assert!(!p.enabled("YUV_SDTV"));
        let features = p.features();
        assert_eq!(features.flags.len(), 3);
        assert_eq!(
            p.constants_module(),
            "const MARBLE_PERIOD = 90.0;\nconst CENTER_2D = vec2f(0.25, 0.75);\n"
        );

        // Rewritten in place of the first pragma line, the others dropped.
        let mut rewritten = text.to_string();
        let mut q = p.clone();
        q.set_flag("MIXOKLAB_SRGB", false);
        q.set_flag("CENTER_2D", false);
        q.write(&mut rewritten);
        assert_eq!(
            rewritten,
            "// Title\n\
             // wesl-const: MARBLE_PERIOD = 90.0\n\
             import lygia::distort::marble::marbleBake1;\n"
        );
        assert_eq!(Pragmas::parse(&rewritten), q);
        // All off: no pragma lines left.
        Pragmas::default().write(&mut rewritten);
        assert_eq!(
            rewritten,
            "// Title\nimport lygia::distort::marble::marbleBake1;\n"
        );
    }

    #[test]
    fn new_pragmas_go_after_the_header() {
        let mut text = String::from("// Title\n// by me\n\nfn f() {}\n");
        let p = Pragmas {
            flags: vec!["YUV_SDTV".into()],
            constants: vec![],
        };
        p.write(&mut text);
        assert_eq!(
            text,
            "// Title\n// by me\n// wesl-flags: YUV_SDTV\n\nfn f() {}\n"
        );
        let mut bare = String::from("fn f() {}");
        p.write(&mut bare);
        assert_eq!(bare, "// wesl-flags: YUV_SDTV\nfn f() {}");
    }

    /// The flags of imported modules and of the modules they import.
    #[test]
    fn usages_follow_imports() {
        let text = "@if(MINE) const X = 1.0;\n\
                    import lygia::sdf::starSDF::starSDF;\n\
                    import lygia::distort::marble::marbleBake1;\n";
        let found = usages(text);
        let names: Vec<&str> = found.iter().map(|u| u.name.as_str()).collect();
        assert!(names.contains(&"MINE"), "{names:?}");
        let center = found
            .iter()
            .find(|u| u.name == "CENTER_2D")
            .expect("CENTER_2D");
        assert!(center.constant);
        assert!(center.users.contains(&"sdf::starSDF"), "{:?}", center.users);
        // starSDF -> space::scale, which tests CENTER_2D too.
        assert!(center.users.contains(&"space::scale"), "{:?}", center.users);
        assert!(!center.notes.is_empty());
        let period = found.iter().find(|u| u.name == "MARBLE_PERIOD").unwrap();
        assert!(period.constant);
        assert_eq!(period.users, ["distort::marble"]);
        assert!(usages("fn f() {}").is_empty());
    }

    /// Ticking a flag in the window writes the pragma and asks for a compile.
    #[test]
    fn checkbox_writes_the_pragma() {
        let ctx = egui::Context::default();
        let mut panel = FlagsPanel::default();
        let mut text = String::from("import lygia::color::space::rgb2yuv::rgb2yuv;\n");
        let mut changed = false;
        let mut run = |panel: &mut FlagsPanel, text: &mut String, events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| changed |= show(ui, panel, text));
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            shapes
        };
        let shapes = run(&mut panel, &mut text, vec![]);
        let label = shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text() == "YUV_SDTV" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("a YUV_SDTV checkbox");
        let click = |pressed| egui::Event::PointerButton {
            pos: label,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run(
            &mut panel,
            &mut text,
            vec![egui::Event::PointerMoved(label)],
        );
        run(&mut panel, &mut text, vec![click(true)]);
        run(&mut panel, &mut text, vec![click(false)]);
        assert!(changed);
        assert_eq!(
            text,
            "// wesl-flags: YUV_SDTV\nimport lygia::color::space::rgb2yuv::rgb2yuv;\n"
        );
    }
}
