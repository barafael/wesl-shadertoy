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
use std::collections::HashMap;
use std::ops::Range;

use bevy::log::{debug, info, warn};
use bevy::platform::time::Instant;
use bevy::prelude::Resource;
use core::time::Duration;
use serde_json::{Value, json};

#[cfg(not(target_arch = "wasm32"))]
mod native;
mod protocol;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
use native::Transport;
#[cfg(target_arch = "wasm32")]
use web::Transport;

use protocol::lsp_position;
pub use protocol::{
    CompletionItem, CompletionKind, Definition, Diagnostic, HintKind, InlayHint, Severity,
    Signature,
};

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

/// Identifies one request the editor made; the answer carries the same one,
/// so the editor can tell its answer from an older one.
pub type Ticket = i64;

/// A server answer to one of the editor's requests.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer<T> {
    pub ticket: Ticket,
    pub value: T,
}

/// What the server last reported or answered.
#[derive(Default)]
struct Results {
    /// On the editor's latest text (carried through edits until the server
    /// reports on it).
    diagnostics: Vec<Diagnostic>,
    /// Like `diagnostics`.
    hints: Vec<InlayHint>,
    completion: Option<Answer<Vec<CompletionItem>>>,
    signature: Option<Answer<Option<Signature>>>,
    /// Taken by the editor once acted on.
    definition: Option<Answer<Option<Definition>>>,
    /// The formatted text, or `None` where the server could not format
    /// (it sends `null` for text it cannot parse). Taken once applied.
    format: Option<Answer<Option<String>>>,
}

/// The language-server connection and its latest results.
#[derive(Resource, Default)]
pub struct Analyzer {
    client: Option<Client<Transport>>,
    status: Status,
    /// Whether to keep inlay hints up to date.
    hints: bool,
}

impl Analyzer {
    pub fn status(&self) -> &Status {
        &self.status
    }

    fn results(&self) -> Option<&Results> {
        self.client.as_ref().map(|c| &c.results)
    }

    /// Diagnostics for the editor text, as the server last saw it.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.results().map_or(&[], |r| &r.diagnostics)
    }

    /// Inlay hints for the editor text, while [`Self::set_inlay_hints`] is on.
    pub fn inlay_hints(&self) -> &[InlayHint] {
        self.results().map_or(&[], |r| &r.hints)
    }

    /// Keep inlay hints up to date (from the next frame on), or drop them.
    pub fn set_inlay_hints(&mut self, on: bool) {
        self.hints = on;
    }

    /// The latest completion list.
    pub fn completion(&self) -> Option<&Answer<Vec<CompletionItem>>> {
        self.results()?.completion.as_ref()
    }

    /// The latest signature help.
    pub fn signature(&self) -> Option<&Answer<Option<Signature>>> {
        self.results()?.signature.as_ref()
    }

    /// The latest definition, once.
    pub fn take_definition(&mut self) -> Option<Answer<Option<Definition>>> {
        self.client.as_mut()?.results.definition.take()
    }

    /// The latest formatting result, once.
    pub fn take_format(&mut self) -> Option<Answer<Option<String>>> {
        self.client.as_mut()?.results.format.take()
    }

    /// Ask for completions at char index `at` of `text` (the editor's
    /// current text). `None` while the server is not ready.
    pub fn request_completion(&mut self, text: &str, at: usize) -> Option<Ticket> {
        self.client
            .as_mut()?
            .at_cursor(text, at, "textDocument/completion", Pending::Completion)
    }

    /// Ask for the signature of the call around char index `at`.
    pub fn request_signature(&mut self, text: &str, at: usize) -> Option<Ticket> {
        self.client
            .as_mut()?
            .at_cursor(text, at, "textDocument/signatureHelp", Pending::Signature)
    }

    /// Ask where the name at char index `at` is defined.
    pub fn request_definition(&mut self, text: &str, at: usize) -> Option<Ticket> {
        self.client
            .as_mut()?
            .at_cursor(text, at, "textDocument/definition", Pending::Definition)
    }

    /// Ask for `text` formatted: all of it, or the lines `selection` (a char
    /// range) touches.
    pub fn request_format(
        &mut self,
        text: &str,
        selection: Option<Range<usize>>,
    ) -> Option<Ticket> {
        self.client.as_mut()?.format(text, selection)
    }

    /// Run one frame: start or stop the server per `enabled`, apply what it
    /// sent, and send `text` if it changed.
    pub fn update(&mut self, enabled: bool, text: &str) {
        if !enabled {
            self.client = None;
            self.status = Status::Off;
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
        client.hints = self.hints;
        match client.step(text) {
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
                self.status = Status::Unavailable(why);
            }
        }
    }
}

/// The one replaced span that turns `old` into `new`, found as their common
/// prefix and suffix: `(start, old_end, new_end)` in chars.
fn edit_span(old: &[char], new: &[char]) -> (usize, usize, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, old.len() - suffix, new.len() - suffix)
}

/// Move `diagnostics` (char ranges into `old`) through the edit that turned
/// `old` into `new` (see [`edit_span`]). Marks after the span shift by the
/// length change; a mark edge inside it snaps to the span's edge, and a mark
/// the edit swallowed whole is dropped.
fn remap(diagnostics: &mut Vec<Diagnostic>, old: &str, new: &str) {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (prefix, old_end, new_end) = edit_span(&old, &new);
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

/// Move inlay hints (char indices into `old`) through the edit that turned
/// `old` into `new`; a hint inside the replaced text is dropped.
fn remap_hints(hints: &mut Vec<InlayHint>, old: &str, new: &str) {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (prefix, old_end, new_end) = edit_span(&old, &new);
    hints.retain_mut(|h| match h.at {
        at if at <= prefix => true,
        at if at >= old_end => {
            h.at = at - old_end + new_end;
            true
        }
        _ => false,
    });
}

/// How long the text must rest before an edit goes to the server: typing
/// sends once per pause, not once per keystroke (each send is a full-text
/// change the server analyzes from scratch). Requests at the cursor send
/// the text at once.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// What a request in flight was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Init,
    Completion,
    Signature,
    Definition,
    Format,
    Hints,
}

/// A request in flight.
struct Request {
    kind: Pending,
    /// The document version and editor text it was made on.
    version: i32,
    text: String,
}

/// The LSP client: initialize handshake, full-text document sync,
/// `publishDiagnostics`, and the requests the editor makes. Anything the
/// server asks gets an empty answer, so it never waits on us.
struct Client<L> {
    link: L,
    next_id: i64,
    pending: HashMap<i64, Request>,
    initialized: bool,
    /// Version and text of the document as the server has it.
    version: i32,
    synced: Option<String>,
    /// The text the results' ranges refer to: the editor's latest, so
    /// marks follow edits at once, before the server sees them.
    seen: Option<String>,
    /// When `seen` last changed, for [`DEBOUNCE`].
    changed: Option<Instant>,
    debounce: Duration,
    /// Whether to keep `results.hints` up to date, and the version they
    /// were last asked for.
    hints: bool,
    hints_version: Option<i32>,
    results: Results,
}

impl<L: Link> Client<L> {
    fn new(link: L) -> Self {
        Self {
            link,
            next_id: 1,
            pending: HashMap::new(),
            initialized: false,
            version: 0,
            synced: None,
            seen: None,
            changed: None,
            debounce: DEBOUNCE,
            hints: false,
            hints_version: None,
            results: Results::default(),
        }
    }

    /// Returns whether the server is initialized; `Err` when it is gone.
    fn step(&mut self, text: &str) -> Result<bool, String> {
        self.step_at(text, Instant::now())
    }

    /// [`Self::step`] at time `now`.
    fn step_at(&mut self, text: &str, now: Instant) -> Result<bool, String> {
        match self.link.state() {
            LinkState::Starting => return Ok(false),
            LinkState::Failed(why) => return Err(why),
            LinkState::Ready => {}
        }
        if !self.initialized && !self.pending.values().any(|r| r.kind == Pending::Init) {
            self.initialize();
        }
        for message in self.link.recv() {
            self.handle(message)?;
        }
        if !self.initialized {
            return Ok(false);
        }
        self.observe(text, now);
        // The document opens at once; edits go out once typing pauses.
        let settled = self
            .changed
            .is_none_or(|at| now.saturating_duration_since(at) >= self.debounce);
        if self.synced.as_deref() != Some(text) && (self.synced.is_none() || settled) {
            self.sync(text);
        }
        if !self.hints {
            self.results.hints.clear();
            self.hints_version = None;
        } else if self.hints_version != Some(self.version)
            && let Some(synced) = self.synced.clone()
        {
            let end = lsp_position(&synced, synced.chars().count());
            let params = json!({
                "textDocument": { "uri": self.uri() },
                "range": { "start": { "line": 0, "character": 0 }, "end": end },
            });
            self.request(Pending::Hints, "textDocument/inlayHint", params, synced);
            self.hints_version = Some(self.version);
        }
        Ok(true)
    }

    fn initialize(&mut self) {
        let root = self.link.root_uri();
        let params = json!({
            "processId": process_id(),
            "clientInfo": { "name": "wesl-shadertoy" },
            "rootUri": root,
            "workspaceFolders": [{ "uri": root, "name": "wesl-shadertoy" }],
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "textDocument": {
                    "synchronization": {},
                    "publishDiagnostics": { "versionSupport": true },
                    "completion": { "completionItem": {
                        "snippetSupport": false, "labelDetailsSupport": true,
                    } },
                    "signatureHelp": { "signatureInformation": {
                        "parameterInformation": { "labelOffsetSupport": true },
                        "activeParameterSupport": true,
                    } },
                    "definition": {},
                    "inlayHint": {},
                    "formatting": {},
                    "rangeFormatting": {},
                },
                "workspace": { "configuration": true },
            },
            // The editor shows type hints only (signature help covers
            // parameter names), and of a `var` the type it holds: `f32`,
            // not `ref<function, f32, read_write>`.
            "initializationOptions": { "inlayHints": {
                "parameterHints": false, "typeVerbosity": "inner",
            } },
        });
        self.request(Pending::Init, "initialize", params, String::new());
    }

    /// Note the editor's text: until the server reports on new text, keep
    /// the old marks on the characters they were about, not on the same
    /// indices.
    fn observe(&mut self, text: &str, now: Instant) {
        if self.seen.as_deref() == Some(text) {
            return;
        }
        if let Some(old) = &self.seen {
            remap(&mut self.results.diagnostics, old, text);
            remap_hints(&mut self.results.hints, old, text);
        }
        self.seen = Some(text.to_owned());
        self.changed = Some(now);
    }

    /// Bring the server up to `text` now, skipping the debounce, so a
    /// request's positions refer to what it has. `false` before the
    /// handshake is done.
    fn flush(&mut self, text: &str) -> bool {
        if !self.initialized {
            return false;
        }
        self.observe(text, Instant::now());
        if self.synced.as_deref() != Some(text) {
            self.sync(text);
        }
        true
    }

    /// A `textDocument/*` request at char index `at` of `text`.
    fn at_cursor(&mut self, text: &str, at: usize, method: &str, kind: Pending) -> Option<Ticket> {
        if !self.flush(text) {
            return None;
        }
        let params = json!({
            "textDocument": { "uri": self.uri() },
            "position": lsp_position(text, at),
        });
        Some(self.request(kind, method, params, text.to_owned()))
    }

    fn format(&mut self, text: &str, selection: Option<Range<usize>>) -> Option<Ticket> {
        if !self.flush(text) {
            return None;
        }
        let options = json!({ "tabSize": 4, "insertSpaces": true });
        let document = json!({ "uri": self.uri() });
        let (method, params) = match selection.filter(|s| !s.is_empty()) {
            Some(s) => (
                "textDocument/rangeFormatting",
                json!({
                    "textDocument": document,
                    "range": { "start": lsp_position(text, s.start), "end": lsp_position(text, s.end) },
                    "options": options,
                }),
            ),
            None => (
                "textDocument/formatting",
                json!({ "textDocument": document, "options": options }),
            ),
        };
        Some(self.request(Pending::Format, method, params, text.to_owned()))
    }

    fn handle(&mut self, message: Value) -> Result<(), String> {
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
            // A response to one of ours.
            (Some(id), None) => {
                let Some(ticket) = id.as_i64() else {
                    return Ok(());
                };
                if let Some(request) = self.pending.remove(&ticket) {
                    self.answer(ticket, request, &message)?;
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
                    let mut diagnostics: Vec<Diagnostic> = params["diagnostics"]
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .filter_map(|d| protocol::diagnostic(text, d))
                                .collect()
                        })
                        .unwrap_or_default();
                    // Typed since it was sent: carry the marks along.
                    if let Some(seen) = self.seen.as_deref().filter(|s| *s != text) {
                        remap(&mut diagnostics, text, seen);
                    }
                    self.results.diagnostics = diagnostics;
                    // The server has analyzed this version now; hints asked
                    // for before that (right after opening) come back empty.
                    self.hints_version = None;
                }
            }
            (None, Some(method)) => debug!("wgsl-analyzer: {method} {}", message["params"]),
            (None, None) => {}
        }
        Ok(())
    }

    /// Apply the response to `request`.
    fn answer(&mut self, ticket: Ticket, request: Request, message: &Value) -> Result<(), String> {
        let error = message.get("error");
        if let Some(error) = error {
            if request.kind == Pending::Init {
                return Err(format!("initialize failed: {error}"));
            }
            debug!("wgsl-analyzer: {:?} failed: {error}", request.kind);
        }
        let result = &message["result"];
        let unchanged = self.seen.as_deref() == Some(request.text.as_str());
        let results = &mut self.results;
        match request.kind {
            Pending::Init => {
                self.notify("initialized", json!({}));
                self.initialized = true;
            }
            // The list is the scope at the cursor; the editor filters it by
            // the word typed since, so it stays useful while typing on.
            Pending::Completion => {
                results.completion = Some(Answer {
                    ticket,
                    value: protocol::completions(result),
                });
            }
            Pending::Signature => {
                results.signature = Some(Answer {
                    ticket,
                    value: protocol::signature(result),
                });
            }
            // Positions into a text that has moved on are no use.
            Pending::Definition if unchanged => {
                let root = self.link.root_uri();
                results.definition = Some(Answer {
                    ticket,
                    value: protocol::definition(result, &root, &request.text),
                });
            }
            Pending::Format if unchanged => {
                results.format = Some(Answer {
                    ticket,
                    value: protocol::apply_edits(&request.text, result),
                });
            }
            Pending::Hints if request.version == self.version => {
                let mut hints = protocol::inlay_hints(result, &request.text);
                if let Some(seen) = self.seen.as_deref().filter(|s| *s != request.text) {
                    remap_hints(&mut hints, &request.text, seen);
                }
                results.hints = hints;
            }
            Pending::Definition | Pending::Format | Pending::Hints => {}
        }
        Ok(())
    }

    fn uri(&self) -> String {
        format!("{}/{USER_DOC}", self.link.root_uri())
    }

    /// Open the document, or replace its whole text.
    fn sync(&mut self, text: &str) {
        let uri = self.uri();
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

    /// Send a request about `text` (the editor text it refers to).
    fn request(&mut self, kind: Pending, method: &str, params: Value, text: String) -> Ticket {
        let id = self.next_id;
        self.next_id += 1;
        self.link.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        let version = self.version;
        self.pending.insert(
            id,
            Request {
                kind,
                version,
                text,
            },
        );
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
        client.debounce = Duration::ZERO;
        let text = "let a = 1;\n// — é\nlet b: u32 = 1.0;\n";

        assert!(!client.step(text).unwrap());
        assert_eq!(methods(&client.link.sent), ["initialize"]);
        let init_id = client.link.sent[0]["id"].clone();

        client.link.inbox.extend([
            json!({ "jsonrpc": "2.0", "id": 7, "method": "workspace/configuration",
                    "params": { "items": [{}, {}] } }),
            json!({ "jsonrpc": "2.0", "id": init_id, "result": { "capabilities": {} } }),
        ]);
        assert!(client.step(text).unwrap());
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
        client.step(text).unwrap();
        assert_eq!(
            client.results.diagnostics,
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
        client.step(&edited).unwrap();
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
        client.step(&edited).unwrap();
        assert_eq!(
            client.link.sent.len(),
            before,
            "unchanged text is not resent"
        );
        assert_eq!(
            client.results.diagnostics[0].range,
            31..34,
            "stale report applied"
        );
    }

    /// Edits wait for a pause in typing; the marks follow them meanwhile,
    /// and a report on the text sent lands on the text shown.
    #[test]
    fn edits_are_debounced() {
        let mut client = Client::new(Fake::default());
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let text = "let a = 1.0;\n";
        client.step_at(text, t0).unwrap();
        let init_id = client.link.sent[0]["id"].clone();
        client
            .link
            .inbox
            .push_back(json!({ "jsonrpc": "2.0", "id": init_id, "result": {} }));
        client.step_at(text, ms(1)).unwrap();
        assert_eq!(
            methods(&client.link.sent).last(),
            Some(&"textDocument/didOpen")
        );

        // Three keystrokes in quick succession: nothing sent yet.
        let sent = client.link.sent.len();
        for (i, typed) in ["// x\nlet a = 1.0;\n", "// xy\nlet a = 1.0;\n"]
            .iter()
            .enumerate()
        {
            client.step_at(typed, ms(10 + 50 * i as u64)).unwrap();
        }
        let typed = "// xyz\nlet a = 1.0;\n";
        client.step_at(typed, ms(110)).unwrap();
        assert_eq!(client.link.sent.len(), sent, "sent while typing");

        // The server reports on version 1 (`1.0` at line 0, 8..11) now: the
        // mark lands where `1.0` is in the text shown.
        client.link.inbox.push_back(json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": "file:///ws/shaders/user.wesl", "version": 1, "diagnostics": [{
                    "range": { "start": { "line": 0, "character": 8 },
                               "end": { "line": 0, "character": 11 } },
                    "severity": 1, "message": "f32",
                }],
            },
        }));
        client.step_at(typed, ms(200)).unwrap();
        assert_eq!(
            client.link.sent.len(),
            sent,
            "sent before the pause was long"
        );
        let shown: Vec<char> = typed.chars().collect();
        let r = client.results.diagnostics[0].range.clone();
        assert_eq!(shown[r].iter().collect::<String>(), "1.0");

        // 150 ms after the last keystroke it goes out, once.
        client.step_at(typed, ms(260)).unwrap();
        client.step_at(typed, ms(300)).unwrap();
        let changes: Vec<_> = client.link.sent[sent..].iter().collect();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["params"]["contentChanges"][0]["text"], typed);
    }

    /// A client past the handshake, with `text` open, and what it sent.
    fn ready_client(text: &str) -> Client<Fake> {
        let mut client = Client::new(Fake::default());
        let t0 = Instant::now();
        client.step_at(text, t0).unwrap();
        let init_id = client.link.sent[0]["id"].clone();
        client
            .link
            .inbox
            .push_back(json!({ "jsonrpc": "2.0", "id": init_id, "result": {} }));
        client.step_at(text, t0).unwrap();
        assert_eq!(
            methods(&client.link.sent).last(),
            Some(&"textDocument/didOpen")
        );
        client
    }

    fn respond(client: &mut Client<Fake>, ticket: Ticket, result: Value) {
        client
            .link
            .inbox
            .push_back(json!({ "jsonrpc": "2.0", "id": ticket, "result": result }));
        let text = client.seen.clone().unwrap();
        client.step(&text).unwrap();
    }

    /// A request at the cursor sends pending typing first (skipping the
    /// debounce), so its position refers to what the server has; the
    /// answer carries the request's ticket.
    #[test]
    fn requests_flush_typing_and_carry_tickets() {
        let mut client = ready_client("fn f() {}\n");
        // Typed, not sent yet (debounced).
        let typed = "fn f() { sn }\n";
        client.step(typed).unwrap();
        assert_eq!(client.version, 1);

        let ticket = client
            .at_cursor(typed, 11, "textDocument/completion", Pending::Completion)
            .unwrap();
        let sent = &client.link.sent;
        let n = sent.len();
        assert_eq!(sent[n - 2]["method"], "textDocument/didChange");
        assert_eq!(sent[n - 2]["params"]["contentChanges"][0]["text"], typed);
        assert_eq!(sent[n - 1]["method"], "textDocument/completion");
        assert_eq!(sent[n - 1]["id"], ticket);
        assert_eq!(
            sent[n - 1]["params"]["position"],
            json!({ "line": 0, "character": 11 })
        );

        respond(
            &mut client,
            ticket,
            json!([{ "label": "snoise3", "kind": 3, "detail": "fn snoise3(v: vec3<f32>) -> f32" }]),
        );
        let answer = client.results.completion.as_ref().unwrap();
        assert_eq!(answer.ticket, ticket);
        assert_eq!(answer.value[0].label, "snoise3");

        // A failed request answers with nothing rather than leaving the
        // editor waiting on the old list.
        let failed = client
            .at_cursor(typed, 11, "textDocument/completion", Pending::Completion)
            .unwrap();
        client
            .link
            .inbox
            .push_back(json!({ "jsonrpc": "2.0", "id": failed,
            "error": { "code": -32603, "message": "boom" } }));
        client.step(typed).unwrap();
        let answer = client.results.completion.as_ref().unwrap();
        assert_eq!((answer.ticket, answer.value.len()), (failed, 0));
    }

    /// Definitions and formatting are positions into the text they were
    /// asked about: once the editor moved on, the answer is dropped.
    #[test]
    fn stale_definitions_and_formats_are_dropped() {
        let text = "let a = 1;\nlet b = a;\n";
        let mut client = ready_client(text);
        let location = json!([{ "uri": "file:///ws/shaders/user.wesl", "range": {
            "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 5 } } }]);

        let ticket = client
            .at_cursor(text, 19, "textDocument/definition", Pending::Definition)
            .unwrap();
        respond(&mut client, ticket, location.clone());
        let answer = client.results.definition.take().unwrap();
        assert_eq!(answer.value, Some(Definition::User(4..5)));

        let ticket = client
            .at_cursor(text, 19, "textDocument/definition", Pending::Definition)
            .unwrap();
        client.step(&format!("{text}// typed\n")).unwrap();
        respond(&mut client, ticket, location);
        assert_eq!(client.results.definition, None, "applied to moved text");

        let text = client.seen.clone().unwrap();
        let ticket = client.format(&text, None).unwrap();
        assert_eq!(
            client.link.sent.last().unwrap()["method"],
            "textDocument/formatting"
        );
        let edit = json!([{ "range": { "start": { "line": 2, "character": 0 },
            "end": { "line": 2, "character": 2 } }, "newText": "//" }]);
        respond(&mut client, ticket, edit);
        assert_eq!(client.results.format.take().unwrap().value.unwrap(), text);
        // Range formatting for a selection; `null` (unparsable) is `None`.
        let ticket = client.format(&text, Some(0..3)).unwrap();
        assert_eq!(
            client.link.sent.last().unwrap()["method"],
            "textDocument/rangeFormatting"
        );
        respond(&mut client, ticket, Value::Null);
        assert_eq!(client.results.format.take().unwrap().value, None);
    }

    /// With hints on, every synced version is asked for once; a reply for
    /// an older version is dropped, and hints follow typing meanwhile.
    #[test]
    fn inlay_hints_follow_versions() {
        let text = "let st = 1.0;\n";
        let mut client = ready_client(text);
        client.debounce = Duration::ZERO;
        client.hints = true;
        client.step(text).unwrap();
        let request = client.link.sent.last().unwrap().clone();
        assert_eq!(request["method"], "textDocument/inlayHint");
        assert_eq!(
            request["params"]["range"]["end"],
            json!({ "line": 1, "character": 0 })
        );
        client.step(text).unwrap();
        assert_eq!(client.link.sent.last().unwrap(), &request, "asked twice");

        let hint =
            json!([{ "position": { "line": 0, "character": 6 }, "label": ": f32", "kind": 1 }]);
        let first = request["id"].as_i64().unwrap();
        respond(&mut client, first, hint.clone());
        assert_eq!(client.results.hints[0].at, 6);

        // Typing before the hint moves it at once; the new version is asked.
        let edited = format!("// x\n{text}");
        client.step(&edited).unwrap();
        assert_eq!(client.results.hints[0].at, 11);
        let second = client.link.sent.last().unwrap()["id"].as_i64().unwrap();
        assert_ne!(second, first);
        // The reply for the first version arriving late is dropped.
        client.pending.insert(
            first,
            Request {
                kind: Pending::Hints,
                version: 1,
                text: text.into(),
            },
        );
        respond(&mut client, first, json!([]));
        assert_eq!(client.results.hints.len(), 1, "stale reply applied");

        // Diagnostics for this version mean the server has analyzed it:
        // hints asked before that (an empty answer) are asked again.
        let asked = client.link.sent.len();
        client.link.inbox.push_back(json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": "file:///ws/shaders/user.wesl", "version": client.version, "diagnostics": [],
            },
        }));
        client.step(&edited).unwrap();
        let again = &client.link.sent[asked..];
        assert_eq!(methods(again), ["textDocument/inlayHint"]);

        client.hints = false;
        client.step(&edited).unwrap();
        assert!(client.results.hints.is_empty());
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

        // The editor's requests, on an example that imports LYGIA.
        let code = crate::examples::by_name("LYGIA 101").unwrap().code;
        let at = |needle: &str| {
            let byte = code.find(needle).unwrap_or_else(|| panic!("{needle}"));
            code[..byte].chars().count()
        };
        analyzer.set_inlay_hints(true);
        wait_for(&mut analyzer, code, &|a| {
            a.inlay_hints().iter().any(|h| h.kind == HintKind::Type)
        });

        let ticket = analyzer.request_completion(code, at("i_time();")).unwrap();
        wait_for(&mut analyzer, code, &|a| {
            a.completion().is_some_and(|c| c.ticket == ticket)
        });
        let items = &analyzer.completion().unwrap().value;
        let snoise = items
            .iter()
            .find(|i| i.label == "snoise3")
            .expect("snoise3");
        assert_eq!(snoise.kind, CompletionKind::Function);
        assert_eq!(
            snoise.detail.as_deref(),
            Some("fn snoise3(v: vec3<f32>) -> f32")
        );

        let ticket = analyzer
            .request_signature(code, at("RING_RADIUS, 0.02"))
            .unwrap();
        wait_for(&mut analyzer, code, &|a| {
            a.signature().is_some_and(|s| s.ticket == ticket)
        });
        let signature = analyzer
            .signature()
            .unwrap()
            .value
            .clone()
            .expect("in a call");
        assert!(signature.label.starts_with("fn stroke("), "{signature:?}");
        assert_eq!(signature.active, Some(1));

        let definition = |analyzer: &mut Analyzer, needle: &str| {
            let ticket = analyzer.request_definition(code, at(needle) + 1).unwrap();
            wait_for(analyzer, code, &|a| {
                a.results()
                    .and_then(|r| r.definition.as_ref())
                    .is_some_and(|d| d.ticket == ticket)
            });
            analyzer.take_definition().and_then(|answer| answer.value)
        };
        let Some(Definition::Module { title, lygia, .. }) =
            definition(&mut analyzer, "snoise3(vec3")
        else {
            panic!("snoise3 resolves into LYGIA");
        };
        assert_eq!(title, "lygia::generative::snoise");
        assert_eq!(lygia.as_deref(), Some("generative::snoise"));
        assert!(matches!(
            definition(&mut analyzer, "i_time();"),
            Some(Definition::Module { lygia: None, .. })
        ));
        assert_eq!(
            definition(&mut analyzer, "res.y;"),
            Some(Definition::User(at("res = ")..at("res = ") + 3))
        );

        // The examples are formatted: formatting each changes nothing.
        for example in crate::examples::ALL {
            let ticket = analyzer.request_format(example.code, None).unwrap();
            wait_for(&mut analyzer, example.code, &|a| {
                a.results()
                    .and_then(|r| r.format.as_ref())
                    .is_some_and(|f| f.ticket == ticket)
            });
            let formatted = analyzer
                .take_format()
                .unwrap()
                .value
                .unwrap_or_else(|| panic!("{} formats", example.name));
            assert!(
                formatted == example.code,
                "{} is not formatted: run the formatter over examples/",
                example.name
            );
        }
    }
}
