//! LSP payloads to and from the app's types: positions (UTF-16 columns vs.
//! egui's char indices), and the results of the requests the editor makes.
//!
//! Everything here is pure: each function takes the text a message refers
//! to, so it can be tested without a server.

use std::ops::Range;

use serde_json::{Value, json};

use super::{RESERVED_MODULES, USER_DOC};

/// How serious a [`Diagnostic`] is (LSP hints count as info).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One server diagnostic on the editor text.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    /// Char range in the document (egui `CCursor` indices), never empty.
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// What a completion candidate is, for its tint in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Variable,
    Constant,
    Field,
    Type,
    Keyword,
    Other,
}

/// One completion candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// The type or signature, e.g. `fn snoise3(v: vec3<f32>) -> f32`.
    pub detail: Option<String>,
    /// What accepting it inserts (in editor terms, see [`from_analyzer`]).
    pub insert: String,
    /// What the typed word is matched against.
    pub filter: String,
    /// The server's order.
    pub sort: String,
}

/// The call the cursor is in, from `textDocument/signatureHelp`.
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    /// e.g. `fn stroke(x: f32, size: f32, w: f32) -> f32`.
    pub label: String,
    /// Each parameter's char range in `label`.
    pub params: Vec<Range<usize>>,
    /// The parameter the cursor is on, if the server knows.
    pub active: Option<usize>,
}

/// Where a definition lives.
#[derive(Clone, Debug, PartialEq)]
pub enum Definition {
    /// In the editor text: this char range of the text the request was made on.
    User(Range<usize>),
    /// In a read-only module: `package::shadertoy` or a LYGIA module.
    Module {
        /// `package::shadertoy`, or `lygia::generative::snoise`.
        title: String,
        /// The LYGIA module path (`generative::snoise`), for the LYGIA window.
        lygia: Option<String>,
        source: &'static str,
        /// Char range in `source`.
        range: Range<usize>,
    },
}

/// What kind of inlay hint the server sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    /// `: vec2<f32>` after a `let`/`var` name.
    Type,
    /// `size:` before a call argument.
    Parameter,
    Other,
}

/// One inlay hint on the editor text.
#[derive(Clone, Debug, PartialEq)]
pub struct InlayHint {
    /// Char index the hint sits at.
    pub at: usize,
    pub label: String,
    pub kind: HintKind,
}

/// The char index of an LSP position (`line`, UTF-16 `column`) in `text`,
/// clamped to the line and the text.
pub fn char_index(text: &str, line: usize, column: usize) -> usize {
    let mut chars = 0;
    for (i, row) in text.split('\n').enumerate() {
        if i == line {
            let mut units = 0;
            for c in row.chars() {
                if units >= column {
                    break;
                }
                units += c.len_utf16();
                chars += 1;
            }
            return chars;
        }
        chars += row.chars().count() + 1;
    }
    text.chars().count()
}

/// The LSP position (`{line, character}`, UTF-16 columns) of char index
/// `at` in `text`, clamped to the end of the text.
pub fn lsp_position(text: &str, at: usize) -> Value {
    let (mut line, mut column) = (0, 0);
    for c in text.chars().take(at) {
        if c == '\n' {
            line += 1;
            column = 0;
        } else {
            column += c.len_utf16();
        }
    }
    json!({ "line": line, "character": column })
}

/// An LSP range as a char range in `text`.
fn char_range(text: &str, range: &Value) -> Option<Range<usize>> {
    let position = |p: &Value| {
        Some(char_index(
            text,
            usize::try_from(p["line"].as_u64()?).ok()?,
            usize::try_from(p["character"].as_u64()?).ok()?,
        ))
    };
    Some(position(&range["start"])?..position(&range["end"])?)
}

/// The text the analyzer sent back, in editor terms: the reverse of
/// `for_analyzer`, so `lygia::filtre::` reads `lygia::filter::` again.
pub fn from_analyzer(text: &str) -> String {
    let mut text = text.to_owned();
    for (name, stand_in) in RESERVED_MODULES {
        let from = format!("lygia::{stand_in}::");
        if text.contains(&from) {
            text = text.replace(&from, &format!("lygia::{name}::"));
        }
    }
    text
}

/// An LSP diagnostic as a [`Diagnostic`] on `text`.
pub fn diagnostic(text: &str, diagnostic: &Value) -> Option<Diagnostic> {
    let Range { mut start, mut end } = char_range(text, &diagnostic["range"])?;
    // Give zero-width reports (e.g. "expected `;`") a visible character.
    if end <= start {
        let len = text.chars().count();
        start = start.min(len.saturating_sub(1));
        end = (start + 1).min(len.max(1));
    }
    let severity = match diagnostic["severity"].as_u64() {
        Some(1) | None => Severity::Error,
        Some(2) => Severity::Warning,
        _ => Severity::Info,
    };
    Some(Diagnostic {
        range: start..end,
        severity,
        message: diagnostic["message"].as_str()?.to_owned(),
    })
}

/// A `textDocument/completion` result (a list or a `CompletionList`).
pub fn completions(result: &Value) -> Vec<CompletionItem> {
    let items = result
        .as_array()
        .or_else(|| result["items"].as_array())
        .map_or(&[][..], Vec::as_slice);
    items
        .iter()
        .filter_map(|item| {
            let label = item["label"].as_str()?.to_owned();
            let text = |key: &str| item[key].as_str().map(str::to_owned);
            let insert = item["textEdit"]["newText"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| text("insertText"))
                .unwrap_or_else(|| label.clone());
            Some(CompletionItem {
                kind: completion_kind(item["kind"].as_u64()),
                detail: text("detail").or_else(|| {
                    item["labelDetails"]["detail"]
                        .as_str()
                        .map(|d| format!("{label}{d}"))
                }),
                insert: from_analyzer(&insert),
                filter: text("filterText").unwrap_or_else(|| label.clone()),
                sort: text("sortText").unwrap_or_else(|| label.clone()),
                label,
            })
        })
        .collect()
}

/// LSP's `CompletionItemKind` numbers, folded to what the list shows.
fn completion_kind(kind: Option<u64>) -> CompletionKind {
    match kind {
        Some(2..=4) => CompletionKind::Function,
        Some(6) => CompletionKind::Variable,
        Some(21) => CompletionKind::Constant,
        Some(5 | 10) => CompletionKind::Field,
        Some(7 | 8 | 13 | 22 | 25) => CompletionKind::Type,
        Some(14) => CompletionKind::Keyword,
        _ => CompletionKind::Other,
    }
}

/// A `textDocument/signatureHelp` result.
pub fn signature(result: &Value) -> Option<Signature> {
    let signatures = result["signatures"].as_array()?;
    let index = result["activeSignature"].as_u64().unwrap_or(0);
    let chosen = signatures.get(usize::try_from(index).ok()?)?;
    let label = chosen["label"].as_str()?.to_owned();
    // UTF-16 offsets into the label, as asked for with
    // `labelOffsetSupport`; a server may still send the parameter's text.
    let char_at = |units: u64| {
        let mut seen = 0;
        label
            .chars()
            .take_while(|c| {
                let before = seen;
                seen += c.len_utf16() as u64;
                before < units
            })
            .count()
    };
    let params = chosen["parameters"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|p| match &p["label"] {
            Value::Array(pair) => {
                Some(char_at(pair.first()?.as_u64()?)..char_at(pair.get(1)?.as_u64()?))
            }
            Value::String(name) => {
                let byte = label.find(name.as_str())?;
                let start = label[..byte].chars().count();
                Some(start..start + name.chars().count())
            }
            _ => None,
        })
        .collect();
    let active = chosen["activeParameter"]
        .as_u64()
        .or_else(|| result["activeParameter"].as_u64())
        .and_then(|a| usize::try_from(a).ok());
    Some(Signature {
        label,
        params,
        active,
    })
}

/// A `textDocument/definition` result, its location resolved against `text`
/// (the editor text the request was made on), the hidden `package::shadertoy`
/// module and the bundled LYGIA sources. `root` is the workspace root URI.
pub fn definition(result: &Value, root: &str, text: &str) -> Option<Definition> {
    let location = match result {
        Value::Array(list) => list.first()?,
        other => other,
    };
    // `Location` or `LocationLink`.
    let uri = location["uri"]
        .as_str()
        .or_else(|| location["targetUri"].as_str())?;
    let range = if location["range"].is_object() {
        &location["range"]
    } else {
        &location["targetSelectionRange"]
    };
    let path = uri.strip_prefix(root)?.trim_start_matches('/');
    if path == USER_DOC {
        return Some(Definition::User(char_range(text, range)?));
    }
    if path == "shaders/shadertoy.wesl" {
        let source = crate::compile::SHADERTOY_MODULE;
        return Some(Definition::Module {
            title: "package::shadertoy".into(),
            lygia: None,
            source,
            range: char_range(source, range)?,
        });
    }
    let module = path.strip_prefix("lygia/")?.strip_suffix(".wesl")?;
    let (first, rest) = module.split_once('/').unwrap_or((module, ""));
    let first = RESERVED_MODULES
        .iter()
        .find(|(_, stand_in)| *stand_in == first)
        .map_or(first, |(name, _)| *name);
    let module = if rest.is_empty() {
        first.to_owned()
    } else {
        format!("{first}::{}", rest.replace('/', "::"))
    };
    let source = crate::compile::lygia_source(&module)?;
    Some(Definition::Module {
        title: format!("lygia::{module}"),
        range: char_range(source, range)?,
        lygia: Some(module),
        source,
    })
}

/// A `textDocument/inlayHint` result on `text`.
pub fn inlay_hints(result: &Value, text: &str) -> Vec<InlayHint> {
    result
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|hint| {
            let position = &hint["position"];
            let at = char_index(
                text,
                usize::try_from(position["line"].as_u64()?).ok()?,
                usize::try_from(position["character"].as_u64()?).ok()?,
            );
            let label = match &hint["label"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts.iter().filter_map(|p| p["value"].as_str()).collect(),
                _ => return None,
            };
            let kind = match hint["kind"].as_u64() {
                Some(1) => HintKind::Type,
                Some(2) => HintKind::Parameter,
                _ => HintKind::Other,
            };
            Some(InlayHint { at, label, kind })
        })
        .collect()
}

/// `text` with a `textDocument/formatting` (or `rangeFormatting`) result's
/// edits applied, in editor terms; `None` when the result is not a list of
/// edits (the server sends `null` for text it cannot parse).
pub fn apply_edits(text: &str, result: &Value) -> Option<String> {
    let edits = result.as_array()?;
    let mut chars: Vec<char> = super::for_analyzer(text).chars().collect();
    let mut spans: Vec<(Range<usize>, &str)> = edits
        .iter()
        .map(|e| Some((char_range(text, &e["range"])?, e["newText"].as_str()?)))
        .collect::<Option<_>>()?;
    // Back to front, so earlier ranges stay valid.
    spans.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, new) in spans {
        let end = range.end.min(chars.len());
        let start = range.start.min(end);
        chars.splice(start..end, new.chars());
    }
    Some(from_analyzer(&chars.into_iter().collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_to_char_indices() {
        // `—` is one UTF-16 unit (three UTF-8 bytes), `𝄞` is two.
        let text = "ab\n// — x\n𝄞y\n";
        assert_eq!(char_index(text, 0, 1), 1);
        assert_eq!(char_index(text, 1, 5), 3 + 5);
        assert_eq!(char_index(text, 1, 99), 3 + 6, "clamped to the line");
        assert_eq!(
            char_index(text, 2, 2),
            3 + 7 + 1,
            "after the surrogate pair"
        );
        assert_eq!(char_index(text, 9, 0), text.chars().count());
    }

    /// `lsp_position` is the inverse of `char_index` at every char.
    #[test]
    fn positions_round_trip() {
        let text = "ab\n// — x\n𝄞y\n\nz";
        for at in 0..=text.chars().count() {
            let p = lsp_position(text, at);
            let back = char_index(
                text,
                p["line"].as_u64().unwrap() as usize,
                p["character"].as_u64().unwrap() as usize,
            );
            assert_eq!(back, at, "{p}");
        }
        assert_eq!(lsp_position(text, 11), json!({ "line": 2, "character": 2 }));
    }

    #[test]
    fn zero_width_diagnostics_get_a_character() {
        let d = json!({
            "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 3 } },
            "message": "expected `;`",
        });
        let got = diagnostic("abc", &d).unwrap();
        assert_eq!(got.range, 2..3);
        assert_eq!(got.severity, Severity::Error);
    }

    #[test]
    fn completion_items() {
        let result = json!({ "isIncomplete": false, "items": [
            { "label": "snoise3", "kind": 3, "detail": "fn snoise3(v: vec3<f32>) -> f32" },
            { "label": "st", "kind": 6, "detail": "vec2<f32>", "sortText": "0" },
            { "label": "edge", "kind": 3,
              "textEdit": { "range": {}, "newText": "lygia::filtre::edge" } },
        ] });
        let items = completions(&result);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, CompletionKind::Function);
        assert_eq!(items[0].insert, "snoise3");
        assert_eq!(items[1].sort, "0");
        assert_eq!(items[1].filter, "st");
        assert_eq!(items[2].insert, "lygia::filter::edge", "renamed back");
        assert_eq!(completions(&json!([{ "label": "x" }])).len(), 1);
        assert!(completions(&Value::Null).is_empty());
    }

    #[test]
    fn signatures() {
        let result = json!({ "signatures": [{
            "label": "fn stroke(x: f32, size: f32, w: f32) -> f32",
            "parameters": [{ "label": [10, 16] }, { "label": [18, 27] }, { "label": [29, 35] }],
        }], "activeParameter": 1 });
        let s = signature(&result).unwrap();
        let chars: Vec<char> = s.label.chars().collect();
        let param = |i: usize| chars[s.params[i].clone()].iter().collect::<String>();
        assert_eq!(param(1), "size: f32");
        assert_eq!(s.active, Some(1));
        // A non-ASCII label: offsets are UTF-16 units, ranges chars.
        let result = json!({ "signatures": [{
            "label": "fn ƒ(𝄞: f32)", "parameters": [{ "label": [5, 12] }] }] });
        let s = signature(&result).unwrap();
        let chars: Vec<char> = s.label.chars().collect();
        assert_eq!(
            chars[s.params[0].clone()].iter().collect::<String>(),
            "𝄞: f32"
        );
        assert_eq!(signature(&Value::Null), None);
        assert_eq!(signature(&json!({ "signatures": [] })), None);
    }

    #[test]
    fn definitions() {
        let root = "file:///ws";
        let text = "let a = 1;\nlet b = a;\n";
        let at = |uri: &str, line: u64, from: u64, to: u64| {
            json!([{ "uri": uri, "range": {
                "start": { "line": line, "character": from },
                "end": { "line": line, "character": to } } }])
        };
        assert_eq!(
            definition(&at("file:///ws/shaders/user.wesl", 0, 4, 5), root, text),
            Some(Definition::User(4..5))
        );
        let Some(Definition::Module {
            title,
            lygia,
            source,
            range,
        }) = definition(
            &at("file:///ws/shaders/shadertoy.wesl", 0, 0, 2),
            root,
            text,
        )
        else {
            panic!("shadertoy");
        };
        assert_eq!((title.as_str(), lygia), ("package::shadertoy", None));
        assert_eq!(&source[..2], "//");
        assert_eq!(range, 0..2);
        let Some(Definition::Module { title, lygia, .. }) = definition(
            &at("file:///ws/lygia/filtre/edge/prewitt.wesl", 0, 0, 1),
            root,
            text,
        ) else {
            panic!("lygia");
        };
        assert_eq!(title, "lygia::filter::edge::prewitt");
        assert_eq!(lygia.as_deref(), Some("filter::edge::prewitt"));
        assert_eq!(definition(&json!([]), root, text), None);
        assert_eq!(definition(&Value::Null, root, text), None);
    }

    #[test]
    fn hints() {
        let text = "let st = 1.0;\nf(1.0);\n";
        let result = json!([
            { "position": { "line": 0, "character": 6 }, "label": ": f32", "kind": 1 },
            { "position": { "line": 1, "character": 2 }, "label": [{ "value": "x" }, { "value": ":" }], "kind": 2 },
        ]);
        assert_eq!(
            inlay_hints(&result, text),
            [
                InlayHint {
                    at: 6,
                    label: ": f32".into(),
                    kind: HintKind::Type
                },
                InlayHint {
                    at: 16,
                    label: "x:".into(),
                    kind: HintKind::Parameter
                },
            ]
        );
    }

    #[test]
    fn formatting_edits() {
        let text = "import lygia::filter::edge::prewitt::edgePrewitt;\nfn f(){return;}\n";
        let edit = |line: u64, from: u64, to: u64, new: &str| {
            json!({ "range": { "start": { "line": line, "character": from },
                               "end": { "line": line, "character": to } }, "newText": new })
        };
        let edits = json!([
            edit(1, 6, 7, " {\n    "),
            edit(1, 14, 15, "\n}"),
            edit(1, 4, 4, "")
        ]);
        assert_eq!(
            apply_edits(text, &edits).unwrap(),
            "import lygia::filter::edge::prewitt::edgePrewitt;\nfn f() {\n    return;\n}\n"
        );
        assert_eq!(apply_edits(text, &json!([])).unwrap(), text);
        assert_eq!(apply_edits(text, &Value::Null), None);
    }
}
