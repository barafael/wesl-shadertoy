//! Live diagnostics from wgsl-analyzer, the WGSL/WESL language server.
//!
//! The server runs out of process and speaks LSP; only the [`Link`] to it
//! differs per platform:
//!
//! * native: the `wgsl-analyzer` executable (`$WGSL_ANALYZER`, else from
//!   `PATH`) over stdio, its workspace in a temp directory ([`native`]);
//! * web: the `wgsl-analyzer-web` package (the same server compiled to wasm
//!   with Emscripten) in a Web Worker, driven through `web/analyzer.js`
//!   ([`web`]). It needs a cross-origin isolated page, see `index.html`.
//!
//! The workspace mirrors what [`crate::compile`] links: `shaders/shadertoy.wesl`
//! holds the hidden `package::shadertoy` module, `lygia/` the bundled LYGIA
//! package (a path dependency in `wesl.toml`), and the editor text is the
//! open document `shaders/user.wesl`, i.e. `package::user`. Imports resolve
//! the same way as in the real compile, and diagnostics arrive in editor
//! coordinates (as UTF-16 columns, converted to egui's char indices here).

use std::borrow::Cow;
use std::ops::Range;

use bevy::log::{debug, info, warn};
use bevy::prelude::Resource;
use serde_json::{Value, json};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
use native::Transport;
#[cfg(target_arch = "wasm32")]
use web::Transport;

/// The open document holding the editor text, relative to the workspace root.
const USER_DOC: &str = "shaders/user.wesl";

/// Files every server starts with, relative to its workspace root: the
/// hidden `package::shadertoy` module, and the bundled LYGIA as the path
/// dependency `lygia/`, so `lygia::` imports resolve as in the real compile.
fn workspace_files() -> Vec<(String, &'static str)> {
    let manifest = "edition = \"2026_pre\"\n\n[dependencies]\nlygia = { path = \"lygia\" }\n";
    let mut files = vec![
        ("wesl.toml".to_string(), manifest),
        (
            "shaders/shadertoy.wesl".to_string(),
            crate::compile::SHADERTOY_MODULE,
        ),
        (
            "lygia/wesl.toml".to_string(),
            "edition = \"2026_pre\"\nroot = \".\"\n",
        ),
    ];
    for (path, source) in crate::compile::lygia_files() {
        files.push((format!("lygia/{}", renamed_module(&path)), source));
    }
    files
}

/// LYGIA modules whose names are WGSL reserved words, with a stand-in of the
/// same length. The WESL spec allows reserved words as module names
/// (wesl-spec#127) and the compiler accepts them, but wgsl-analyzer 0.12
/// still rejects them, so the workspace mirror and the text it is sent use
/// the stand-in. Same length, so diagnostic offsets need no remapping.
const RESERVED_MODULES: &[(&str, &str)] = &[("filter", "filtre")];

/// `path` (relative to the LYGIA root) with reserved module names swapped.
fn renamed_module(path: &str) -> String {
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    match RESERVED_MODULES.iter().find(|(name, _)| *name == first) {
        Some((_, stand_in)) if !rest.is_empty() => format!("{stand_in}/{rest}"),
        _ => path.to_string(),
    }
}

/// The editor text as the analyzer sees it: `lygia::filter::…` imports point
/// at the renamed mirror (see [`RESERVED_MODULES`]).
fn for_analyzer(text: &str) -> Cow<'_, str> {
    let mut text = Cow::Borrowed(text);
    for (name, stand_in) in RESERVED_MODULES {
        let from = format!("lygia::{name}::");
        if text.contains(&from) {
            text = Cow::Owned(text.replace(&from, &format!("lygia::{stand_in}::")));
        }
    }
    text
}

/// A connection to a language server, as the [`Client`] needs it.
trait Link {
    /// Whether the server can take messages yet.
    fn state(&mut self) -> LinkState;
    /// `file://` URI of the workspace root.
    fn root_uri(&self) -> String;
    /// Send one JSON-RPC message.
    fn send(&mut self, message: &Value);
    /// Messages received since the last call.
    fn recv(&mut self) -> Vec<Value>;
}

enum LinkState {
    /// Loading the server (web) or writing its workspace (native).
    Starting,
    Ready,
    Failed(String),
}

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

/// What the analyzer is doing, for the status bar and settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Turned off in the settings.
    #[default]
    Off,
    /// Loading or initializing the server.
    Starting,
    Ready,
    /// Could not start or stopped working; why. Retried when the setting is
    /// toggled off and on again.
    Unavailable(String),
}

/// The language-server connection and its latest diagnostics.
#[derive(Resource, Default)]
pub struct Analyzer {
    client: Option<Client<Transport>>,
    status: Status,
    diagnostics: Vec<Diagnostic>,
}

impl Analyzer {
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Diagnostics for the editor text the server last saw.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Run one frame: start or stop the server per `enabled`, apply what it
    /// sent, and send `text` if it changed.
    pub fn update(&mut self, enabled: bool, text: &str) {
        if !enabled {
            self.client = None;
            self.status = Status::Off;
            self.diagnostics.clear();
            return;
        }
        if self.client.is_none() {
            if matches!(self.status, Status::Unavailable(_)) {
                return;
            }
            match Transport::start(workspace_files()) {
                Ok(link) => {
                    self.client = Some(Client::new(link));
                    self.status = Status::Starting;
                }
                Err(why) => {
                    warn!("wgsl-analyzer unavailable: {why}");
                    self.status = Status::Unavailable(why);
                    return;
                }
            }
        }
        let Some(client) = self.client.as_mut() else {
            return;
        };
        match client.step(text, &mut self.diagnostics) {
            Ok(true) => {
                if self.status != Status::Ready {
                    info!("wgsl-analyzer ready");
                }
                self.status = Status::Ready;
            }
            Ok(false) => self.status = Status::Starting,
            Err(why) => {
                warn!("wgsl-analyzer stopped: {why}");
                self.client = None;
                self.diagnostics.clear();
                self.status = Status::Unavailable(why);
            }
        }
    }
}

/// Move `diagnostics` (char ranges into `old`) through the edit that turned
/// `old` into `new`: one replaced span, found as the common prefix and suffix.
/// Marks after the span shift by the length change; a mark edge inside it
/// snaps to the span's edge, and a mark the edit swallowed whole is dropped.
fn remap(diagnostics: &mut Vec<Diagnostic>, old: &str, new: &str) {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
    // A start at the edit's end follows inserted text; an end at the edit's
    // start stays put, so typing next to a mark never stretches it.
    let start = |i: usize| match i {
        i if i >= old_end => i - old_end + new_end,
        i if i <= prefix => i,
        _ => new_end,
    };
    let end = |i: usize| match i {
        i if i <= prefix => i,
        i if i >= old_end => i - old_end + new_end,
        _ => prefix,
    };
    diagnostics.retain_mut(|d| {
        d.range = start(d.range.start)..end(d.range.end);
        !d.range.is_empty()
    });
}

/// The LSP client: initialize handshake, full-text document sync, and
/// `publishDiagnostics` for the editor document. Anything else the server
/// asks for gets an empty answer, so it never waits on us.
struct Client<L> {
    link: L,
    next_id: i64,
    /// The pending `initialize` request.
    init_id: Option<i64>,
    initialized: bool,
    /// Version and text of the document as the server has it.
    version: i32,
    synced: Option<String>,
}

impl<L: Link> Client<L> {
    fn new(link: L) -> Self {
        Self {
            link,
            next_id: 1,
            init_id: None,
            initialized: false,
            version: 0,
            synced: None,
        }
    }

    /// Returns whether the server is initialized; `Err` when it is gone.
    fn step(&mut self, text: &str, diagnostics: &mut Vec<Diagnostic>) -> Result<bool, String> {
        match self.link.state() {
            LinkState::Starting => return Ok(false),
            LinkState::Failed(why) => return Err(why),
            LinkState::Ready => {}
        }
        if !self.initialized && self.init_id.is_none() {
            let root = self.link.root_uri();
            let id = self.request(
                "initialize",
                json!({
                    "processId": process_id(),
                    "clientInfo": { "name": "wesl-shadertoy" },
                    "rootUri": root,
                    "workspaceFolders": [{ "uri": root, "name": "wesl-shadertoy" }],
                    "capabilities": {
                        "general": { "positionEncodings": ["utf-16"] },
                        "textDocument": {
                            "synchronization": {},
                            "publishDiagnostics": { "versionSupport": true },
                        },
                        "workspace": { "configuration": true },
                    },
                }),
            );
            self.init_id = Some(id);
        }
        for message in self.link.recv() {
            self.handle(message, diagnostics)?;
        }
        if self.initialized && self.synced.as_deref() != Some(text) {
            // Until the server reports on the new text, keep the old marks
            // on the characters they were about, not on the same indices.
            if let Some(old) = &self.synced {
                remap(diagnostics, old, text);
            }
            self.sync(text);
        }
        Ok(self.initialized)
    }

    fn handle(&mut self, message: Value, diagnostics: &mut Vec<Diagnostic>) -> Result<(), String> {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (id, method) {
            // A request from the server.
            (Some(id), Some(method)) => {
                let result = if method == "workspace/configuration" {
                    let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; items])
                } else {
                    Value::Null
                };
                self.link
                    .send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            // A response; only `initialize` matters.
            (Some(id), None) => {
                if self.init_id.is_some() && id.as_i64() == self.init_id {
                    self.init_id = None;
                    if let Some(error) = message.get("error") {
                        return Err(format!("initialize failed: {error}"));
                    }
                    self.notify("initialized", json!({}));
                    self.initialized = true;
                }
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                let params = &message["params"];
                let ours = params["uri"]
                    .as_str()
                    .is_some_and(|uri| uri.ends_with(&format!("/{USER_DOC}")));
                // A report for an older version is about to be superseded.
                let current = params["version"]
                    .as_i64()
                    .is_none_or(|v| v == i64::from(self.version));
                if ours && current {
                    let text = self.synced.as_deref().unwrap_or_default();
                    *diagnostics = params["diagnostics"]
                        .as_array()
                        .map(|list| list.iter().filter_map(|d| convert(text, d)).collect())
                        .unwrap_or_default();
                }
            }
            (None, Some(method)) => debug!("wgsl-analyzer: {method} {}", message["params"]),
            (None, None) => {}
        }
        Ok(())
    }

    /// Open the document, or replace its whole text.
    fn sync(&mut self, text: &str) {
        let uri = format!("{}/{USER_DOC}", self.link.root_uri());
        let sent = for_analyzer(text);
        self.version += 1;
        if self.synced.is_none() {
            self.notify(
                "textDocument/didOpen",
                json!({ "textDocument": {
                    "uri": uri, "languageId": "wesl", "version": self.version, "text": sent,
                } }),
            );
        } else {
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": self.version },
                    "contentChanges": [{ "text": sent }],
                }),
            );
        }
        self.synced = Some(text.to_owned());
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.link.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.link
            .send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }
}

/// Lets a native server exit if the app dies without shutting it down.
fn process_id() -> Value {
    #[cfg(not(target_arch = "wasm32"))]
    {
        json!(std::process::id())
    }
    #[cfg(target_arch = "wasm32")]
    {
        Value::Null
    }
}

/// An LSP diagnostic as a [`Diagnostic`] on `text`.
fn convert(text: &str, diagnostic: &Value) -> Option<Diagnostic> {
    let range = &diagnostic["range"];
    let position = |p: &Value| {
        Some(char_index(
            text,
            usize::try_from(p["line"].as_u64()?).ok()?,
            usize::try_from(p["character"].as_u64()?).ok()?,
        ))
    };
    let (mut start, mut end) = (position(&range["start"])?, position(&range["end"])?);
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

/// The char index of an LSP position (`line`, UTF-16 `column`) in `text`,
/// clamped to the line and the text.
fn char_index(text: &str, line: usize, column: usize) -> usize {
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    /// Reserved-word LYGIA modules are renamed alike in the mirror and the
    /// text, keeping the text's length so diagnostic offsets still line up.
    #[test]
    fn reserved_lygia_modules_are_renamed() {
        assert_eq!(
            renamed_module("filter/edge/prewitt.wesl"),
            "filtre/edge/prewitt.wesl"
        );
        assert_eq!(renamed_module("filter.wesl"), "filter.wesl");
        assert_eq!(renamed_module("math/filter.wesl"), "math/filter.wesl");
        let text = "import lygia::filter::edge::prewitt::edgePrewitt;\nfn filter() {}";
        let sent = for_analyzer(text);
        assert_eq!(sent.len(), text.len());
        assert!(sent.contains("lygia::filtre::edge") && sent.contains("fn filter()"));
        let files = workspace_files();
        assert!(
            files
                .iter()
                .any(|(p, _)| p == "lygia/filtre/edge/prewitt.wesl")
        );
        assert!(!files.iter().any(|(p, _)| p.starts_with("lygia/filter/")));
    }

    /// Marks follow their characters through an edit until the server
    /// reports on the new text.
    #[test]
    fn diagnostics_follow_edits() {
        let mark = |range| Diagnostic {
            range,
            severity: Severity::Error,
            message: String::new(),
        };
        let old = "let t: u32 = 1.0;";
        let remapped = |new: &str, range| {
            let mut d = vec![mark(range)];
            remap(&mut d, old, new);
            d.first().map(|d| d.range.clone())
        };
        // `1.0` is 13..16.
        assert_eq!(
            remapped("    let t: u32 = 1.0;", 13..16),
            Some(17..20),
            "insert before"
        );
        assert_eq!(
            remapped("let t: u32 = 1.0; // x", 13..16),
            Some(13..16),
            "insert after"
        );
        assert_eq!(
            remapped("let t: u32 = 1.0f;", 13..16),
            Some(13..16),
            "typed at its end"
        );
        assert_eq!(
            remapped("let t: u32 = -1.0;", 13..16),
            Some(14..17),
            "typed at its start"
        );
        assert_eq!(
            remapped("let t: u32 = 1.50;", 13..16),
            Some(13..17),
            "typed inside"
        );
        assert_eq!(
            remapped("let t: u32 = 1;", 13..16),
            Some(13..14),
            "partly deleted"
        );
        assert_eq!(remapped("let t: u32 = ;", 13..16), None, "deleted");
        assert_eq!(
            remapped("let t: i32 = 1.0;", 7..10),
            Some(7..10),
            "replaced in place"
        );
    }

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

    /// Scripted server: records what the client sends, replays queued
    /// messages on `recv`.
    #[derive(Default)]
    struct Fake {
        sent: Vec<Value>,
        inbox: VecDeque<Value>,
    }

    impl Link for Fake {
        fn state(&mut self) -> LinkState {
            LinkState::Ready
        }
        fn root_uri(&self) -> String {
            "file:///ws".into()
        }
        fn send(&mut self, message: &Value) {
            self.sent.push(message.clone());
        }
        fn recv(&mut self) -> Vec<Value> {
            self.inbox.drain(..).collect()
        }
    }

    fn methods(sent: &[Value]) -> Vec<&str> {
        sent.iter().filter_map(|m| m["method"].as_str()).collect()
    }

    #[test]
    fn handshake_sync_and_diagnostics() {
        let mut client = Client::new(Fake::default());
        let mut diags = Vec::new();
        let text = "let a = 1;\n// — é\nlet b: u32 = 1.0;\n";

        assert!(!client.step(text, &mut diags).unwrap());
        assert_eq!(methods(&client.link.sent), ["initialize"]);
        let init_id = client.link.sent[0]["id"].clone();

        client.link.inbox.extend([
            json!({ "jsonrpc": "2.0", "id": 7, "method": "workspace/configuration",
                    "params": { "items": [{}, {}] } }),
            json!({ "jsonrpc": "2.0", "id": init_id, "result": { "capabilities": {} } }),
        ]);
        assert!(client.step(text, &mut diags).unwrap());
        let sent = &client.link.sent;
        assert_eq!(sent[1]["id"], 7);
        assert_eq!(sent[1]["result"], json!([null, null]));
        assert_eq!(
            methods(sent),
            ["initialize", "initialized", "textDocument/didOpen"]
        );
        assert_eq!(
            sent[3]["params"]["textDocument"]["uri"],
            "file:///ws/shaders/user.wesl"
        );
        assert_eq!(sent[3]["params"]["textDocument"]["text"], text);

        // Line 2, UTF-16 columns 13..16 is `1.0`; lines 0-1 hold 18 chars.
        let report = |version: i32, line: u64| {
            json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": "file:///ws/shaders/user.wesl", "version": version, "diagnostics": [{
                    "range": { "start": { "line": line, "character": 13 },
                               "end": { "line": line, "character": 16 } },
                    "severity": 1, "message": "expected u32, found f32",
                }],
            } })
        };
        client.link.inbox.push_back(report(1, 2));
        client.step(text, &mut diags).unwrap();
        assert_eq!(
            diags,
            [Diagnostic {
                range: 31..34,
                severity: Severity::Error,
                message: "expected u32, found f32".into(),
            }]
        );
        let doc: Vec<char> = text.chars().collect();
        assert_eq!(doc[31..34].iter().collect::<String>(), "1.0");

        // An edit goes out as a full-text change.
        let edited = format!("{text}\n");
        client.step(&edited, &mut diags).unwrap();
        let change = client.link.sent.last().unwrap();
        assert_eq!(change["method"], "textDocument/didChange");
        assert_eq!(change["params"]["textDocument"]["version"], 2);
        assert_eq!(
            change["params"]["contentChanges"][0]["text"],
            edited.as_str()
        );
        // A report for version 1 arriving now is stale and ignored.
        client.link.inbox.push_back(report(1, 0));
        let before = client.link.sent.len();
        client.step(&edited, &mut diags).unwrap();
        assert_eq!(
            client.link.sent.len(),
            before,
            "unchanged text is not resent"
        );
        assert_eq!(diags[0].range, 31..34, "stale report applied");
    }

    #[test]
    fn zero_width_diagnostics_get_a_character() {
        let d = json!({
            "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 3 } },
            "message": "expected `;`",
        });
        let got = convert("abc", &d).unwrap();
        assert_eq!(got.range, 2..3);
        assert_eq!(got.severity, Severity::Error);
    }

    /// End to end against a real server. Needs the `wgsl-analyzer` binary:
    /// `WGSL_ANALYZER=/path/to/wgsl-analyzer cargo test -- --ignored`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "needs a wgsl-analyzer binary (set WGSL_ANALYZER)"]
    fn native_server_checks_user_code() {
        let mut analyzer = Analyzer::default();
        let wait_for = |analyzer: &mut Analyzer, text: &str, done: &dyn Fn(&Analyzer) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !done(analyzer) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "timed out: {:?}",
                    analyzer.status
                );
                analyzer.update(true, text);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };

        // Every example is clean (imports from package::shadertoy resolve).
        for example in crate::examples::ALL {
            wait_for(&mut analyzer, example.code, &|a| {
                a.status == Status::Ready
                    && a.client.as_ref().and_then(|c| c.synced.as_deref()) == Some(example.code)
            });
            // Give the server time to publish for this version.
            for _ in 0..50 {
                analyzer.update(true, example.code);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(
                analyzer.diagnostics(),
                [],
                "{} should be clean",
                example.name
            );
        }

        let broken = "import package::shadertoy::i_time;\n\
                      fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {\n\
                      \x20   let t: u32 = i_time();\n\
                      \x20   return vec4<f32>(1.0);\n\
                      }\n";
        wait_for(&mut analyzer, broken, &|a| !a.diagnostics().is_empty());
        let d = &analyzer.diagnostics()[0];
        assert_eq!(d.severity, Severity::Error);
        let chars: Vec<char> = broken.chars().collect();
        let flagged: String = chars[d.range.clone()].iter().collect();
        assert_eq!(flagged, "i_time()", "{d:?}");
    }
}
