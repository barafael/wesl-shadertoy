//! Share links: the whole document in the URL fragment, so a link *is* the
//! shader and no server is involved.
//!
//! The editor text (with any tweak not yet written back applied first) is
//! deflated, base64url-encoded and put behind a format version:
//! `#code=1.<data>`. A document that is a built-in example verbatim gets
//! the short `#example=<name>` instead. The fragment never reaches a server,
//! and GitHub Pages serves the same page whatever it holds.
//!
//! At startup a link wins over the saved document, since opening one is an
//! explicit request. The saved document is not touched: the autosave only
//! writes once the user edits, and if the saved work differs from the link
//! the "Shared link" window offers to go back to it (see [`show_opened`]).
//! Web: the fragment is then removed from the address bar, so reloading
//! after an edit shows the edit rather than the link again. Native:
//! `--open-link <url>` opens a link the same way.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bevy::prelude::*;
use bevy_egui::egui;

use crate::examples::{self, Example};

/// Where native builds point their links: the deployed web build.
pub const PUBLIC_URL: &str = "https://barafael.github.io/wesl-shadertoy/";

/// Links longer than this get a warning: some chat apps and link previews
/// cut them off around here.
pub const LONG_LINK: usize = 8 * 1024;

/// Version of the `code=` payload; bump it when the encoding changes and
/// keep decoding the old ones.
const FORMAT: &str = "1";

/// Largest document a link may inflate to, so a crafted link cannot
/// exhaust memory.
const MAX_DOC: usize = 1 << 20;

/// What a link opens.
pub enum Link {
    Code(String),
    Example(&'static Example),
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Link::Code(text) => f.debug_tuple("Code").field(text).finish(),
            Link::Example(e) => f.debug_tuple("Example").field(&e.name).finish(),
        }
    }
}

/// Examples are equal by name (names are unique).
impl PartialEq for Link {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Link::Code(a), Link::Code(b)) => a == b,
            (Link::Example(a), Link::Example(b)) => a.name == b.name,
            _ => false,
        }
    }
}

/// The fragment (without `#`) that carries `text`.
pub fn fragment(text: &str) -> String {
    if let Some(example) = examples::ALL.iter().find(|e| e.code == text) {
        return format!("example={}", percent_encode(example.name));
    }
    let deflated = miniz_oxide::deflate::compress_to_vec(text.as_bytes(), 9);
    format!("code={FORMAT}.{}", URL_SAFE_NO_PAD.encode(deflated))
}

/// A link to `text` on the page at `base` (any fragment of `base` is dropped).
pub fn link(base: &str, text: &str) -> String {
    let base = base.split('#').next().unwrap_or(base);
    format!("{base}#{}", fragment(text))
}

/// Read a link, or just its fragment (with or without `#`). `Ok(None)` when
/// it carries nothing this app shares (no fragment, or someone else's);
/// `Err` says why a share link could not be opened.
pub fn parse(link: &str) -> Result<Option<Link>, String> {
    let fragment = match link.split_once('#') {
        Some((_, fragment)) => fragment,
        None if link.contains("://") => return Ok(None),
        None => link,
    };
    if let Some(name) = fragment.strip_prefix("example=") {
        let name = percent_decode(name);
        return examples::by_name(&name)
            .map(|e| Some(Link::Example(e)))
            .ok_or_else(|| format!("there is no example called \"{name}\""));
    }
    let Some(payload) = fragment.strip_prefix("code=") else {
        return Ok(None);
    };
    let Some((version, data)) = payload.split_once('.') else {
        return Err("the link is incomplete (no format version)".into());
    };
    if version != FORMAT {
        return Err(format!(
            "the link uses format {version}, which this version cannot read; \
             is it from a newer wesl-shadertoy?"
        ));
    }
    let deflated = URL_SAFE_NO_PAD
        .decode(data.trim_end_matches('='))
        .map_err(|_| "the link is damaged (it may have been cut off)".to_string())?;
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(&deflated, MAX_DOC)
        .map_err(|_| "the link is damaged (it may have been cut off)".to_string())?;
    String::from_utf8(bytes)
        .map(|text| Some(Link::Code(text)))
        .map_err(|_| "the link does not hold text".to_string())
}

/// `%`-escape everything but unreserved characters (for example names).
fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Undo `%XX` escapes (and `+` for a space); malformed escapes stay as typed.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --- the app side ---------------------------------------------------------------

/// What the startup link did, shown in the "Shared link" window until it
/// is dismissed.
#[derive(Resource, Default)]
pub struct Shared {
    opened: Option<Opened>,
}

enum Opened {
    /// Opened, while the saved document held this other work.
    Replaced(String),
    Failed(String),
}

/// The document a startup link asks for, with the example it is (if any).
/// Records in `shared` what the window should say about it.
pub fn startup(shared: &mut Shared) -> Option<(String, Option<&'static Example>)> {
    let link = startup_link()?;
    let (text, example) = match parse(&link) {
        Ok(Some(Link::Code(text))) => (text, None),
        Ok(Some(Link::Example(example))) => (example.code.to_string(), Some(example)),
        Ok(None) => return None,
        Err(why) => {
            warn!("could not open the link: {why}");
            shared.opened = Some(Opened::Failed(why));
            return None;
        }
    };
    // The autosave leaves the saved document alone until the first edit;
    // the window offers the way back before that happens.
    let saved = crate::persist::load_doc();
    if let Some(saved) = saved.filter(|s| *s != text && !examples::ALL.iter().any(|e| e.code == s))
    {
        shared.opened = Some(Opened::Replaced(saved));
    }
    Some((text, example))
}

/// The link the page was opened with (web), removing it from the address bar.
#[cfg(target_arch = "wasm32")]
fn startup_link() -> Option<String> {
    let window = web_sys::window()?;
    let location = window.location();
    let hash = location.hash().ok().filter(|h| h.len() > 1)?;
    if let (Ok(path), Ok(search), Ok(history)) =
        (location.pathname(), location.search(), window.history())
    {
        let _ = history.replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&(path + &search)),
        );
    }
    Some(hash)
}

/// `--open-link <url>` (native).
#[cfg(not(target_arch = "wasm32"))]
fn startup_link() -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--open-link" {
            return args.next();
        }
    }
    None
}

/// The page links point at: this page on the web, [`PUBLIC_URL`] natively.
fn base_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let location = web_sys::window().map(|w| w.location());
        if let Some((Ok(origin), Ok(path))) = location.map(|l| (l.origin(), l.pathname())) {
            return origin + &path;
        }
    }
    PUBLIC_URL.to_string()
}

/// The Share button: copy a link to `text` (with the tweaks that are not
/// written back yet applied) and return the status-bar note.
pub fn copy_link(ctx: &egui::Context, text: &str, tweaks: &crate::Tweaks) -> (String, bool) {
    let values = tweaks.values.map(|v| v.to_array());
    let text = crate::tweak::written_back(text, &tweaks.constants, &values, tweaks.dirty);
    let link = link(&base_url(), &text);
    let size = link.len();
    ctx.copy_text(link);
    let kb = (size as f32 / 1024.0).max(0.1);
    if size > LONG_LINK {
        let note = format!(
            "link copied, but it is {kb:.0} KB long: some chat apps cut links over {} KB",
            LONG_LINK / 1024
        );
        (note, false)
    } else {
        (format!("link copied ({kb:.1} KB)"), true)
    }
}

/// The "Shared link" window: why a link failed, or the way back to the
/// saved work a link replaced. Returns that work when the user asks for it.
pub fn show_opened(
    ctx: &egui::Context,
    glass: Option<crate::Glass>,
    shared: &mut Shared,
) -> Option<String> {
    let opened = shared.opened.as_ref()?;
    let mut open = true;
    let mut restore = false;
    let mut dismiss = false;
    crate::show_window(
        ctx,
        glass,
        "shared_link_window",
        egui::Window::new("Shared link")
            .default_pos([12.0, 60.0])
            .default_width(340.0)
            .collapsible(false)
            .open(&mut open),
        |ui| match opened {
            Opened::Replaced(_) => {
                ui.label(
                    "Opened the shader from the link. Your own saved work is \
                     still in browser storage until you edit this one.",
                );
                ui.horizontal(|ui| {
                    restore = ui.button("Back to my saved work").clicked();
                    dismiss = ui.button("Keep the shared shader").clicked();
                });
            }
            Opened::Failed(why) => {
                ui.label(format!("Could not open the link: {why}."));
                dismiss = ui.button("OK").clicked();
            }
        },
    );
    match shared.opened.take() {
        Some(Opened::Replaced(work)) if restore => Some(work),
        other if open && !dismiss => {
            shared.opened = other;
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str =
        "// mine\nfn main_image(p: vec2<f32>) -> vec4<f32> {\n    return vec4<f32>(1.0);\n}\n";

    #[test]
    fn code_roundtrips() {
        let link = link("https://example.org/app/#old", SRC);
        assert!(
            link.starts_with("https://example.org/app/#code=1."),
            "{link}"
        );
        // Only URL-safe characters, so chat apps keep the link in one piece.
        let data = link.split_once("#code=").unwrap().1;
        assert!(
            data.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        );
        assert_eq!(parse(&link), Ok(Some(Link::Code(SRC.into()))));
        assert_eq!(
            parse(&format!("#{}", fragment(SRC))),
            Ok(Some(Link::Code(SRC.into())))
        );
        // Unicode survives, and deflate pays off on real shaders.
        let unicode = "// Größe — ✓\n".repeat(50);
        assert_eq!(
            parse(&fragment(&unicode)),
            Ok(Some(Link::Code(unicode.clone())))
        );
        assert!(fragment(&unicode).len() < unicode.len() / 4);
    }

    #[test]
    fn examples_share_by_name() {
        let plasma = examples::by_name("Plasma").unwrap();
        assert_eq!(fragment(plasma.code), "example=Plasma");
        let spaced = examples::ALL.iter().find(|e| e.name.contains(' ')).unwrap();
        let f = fragment(spaced.code);
        assert!(!f.contains(' '), "{f}");
        assert_eq!(parse(&f), Ok(Some(Link::Example(spaced))));
        assert_eq!(parse("example=plasma"), Ok(Some(Link::Example(plasma))));
        assert!(parse("example=Nope").unwrap_err().contains("Nope"));
    }

    #[test]
    fn versioned() {
        let current = fragment(SRC);
        let newer = current.replacen("code=1.", "code=2.", 1);
        assert!(parse(&newer).unwrap_err().contains("format 2"));
        assert!(parse("code=AAAA").unwrap_err().contains("version"));
    }

    /// Damaged links are an error message, never a panic.
    #[test]
    fn malformed_links_fail_gracefully() {
        let good = fragment(&SRC.repeat(20));
        for bad in [
            "code=1.",
            "code=1.!!!",
            "code=1.AAAA",
            &good[..good.len() / 2],
            &good[..good.len() - 1],
        ] {
            assert!(parse(bad).is_err(), "{bad} parsed");
        }
        // Not ours: ignored.
        assert_eq!(parse("https://example.org/"), Ok(None));
        assert_eq!(parse("https://example.org/#section"), Ok(None));
        assert_eq!(parse(""), Ok(None));
        // A deflate bomb stops at the limit.
        let bomb = miniz_oxide::deflate::compress_to_vec(&vec![b' '; MAX_DOC + 1], 9);
        let bomb = format!("code=1.{}", URL_SAFE_NO_PAD.encode(bomb));
        assert!(parse(&bomb).is_err());
    }

    #[test]
    fn percent_escapes() {
        assert_eq!(percent_encode("Star Nest"), "Star%20Nest");
        assert_eq!(percent_decode("Star%20Nest"), "Star Nest");
        assert_eq!(percent_decode("Star+Nest%"), "Star Nest%");
        assert_eq!(percent_decode("%C3%A9%zz"), "é%zz");
    }
}
