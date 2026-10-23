//! Persistence and file I/O for the document and settings.
//!
//! Web: the document and settings live in `localStorage`. Ctrl+O opens a file
//! through a hidden `<input type=file>` whose `onchange` reads the text into
//! a thread-local queue — JS callbacks cannot touch the Bevy world, so the UI
//! system drains the queue with [`take_opened`] each frame.
//!
//! Native: Ctrl+S writes `shader.wesl` next to the executable and Ctrl+O
//! reads it back; settings are in-memory only.

use bevy_egui::egui::Context;

#[cfg(target_arch = "wasm32")]
const DOC_KEY: &str = "wesl-shadertoy.doc";
#[cfg(target_arch = "wasm32")]
const SETTINGS_KEY: &str = "wesl-shadertoy.settings";

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

// --- document --------------------------------------------------------------

/// The saved document, if the browser has one (native builds start fresh).
#[cfg(target_arch = "wasm32")]
pub fn load_doc() -> Option<String> {
    local_storage()?.get_item(DOC_KEY).ok().flatten()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_doc() -> Option<String> {
    None
}

/// Debounced localStorage save while typing (egui's clock is the timer).
#[cfg(target_arch = "wasm32")]
pub fn save_dirty(ctx: &Context, text: &str) {
    use std::cell::RefCell;
    thread_local! {
        static LAST_SAVE: RefCell<f64> = const { RefCell::new(0.0) };
    }
    let now = ctx.input(|i| i.time);
    if now - LAST_SAVE.with(|t| *t.borrow()) > 0.5 {
        LAST_SAVE.with(|t| *t.borrow_mut() = now);
        if let Some(ls) = local_storage() {
            let _ = ls.set_item(DOC_KEY, text);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save_dirty(_ctx: &Context, _text: &str) {}

/// Ctrl+S: save right now. `Ok` carries a human-readable location.
#[cfg(target_arch = "wasm32")]
pub fn save_now(text: &str) -> Result<String, String> {
    local_storage()
        .and_then(|ls| ls.set_item(DOC_KEY, text).ok())
        .map(|_| "browser storage".to_string())
        .ok_or_else(|| "localStorage unavailable".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save_now(text: &str) -> Result<String, String> {
    let path = native_doc_path()?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Ctrl+O: ask for a document. `Ok(Some(text))` when loaded synchronously
/// (native), `Ok(None)` when a dialog was opened (web — the text arrives
/// later through [`take_opened`]).
#[cfg(target_arch = "wasm32")]
pub fn open_now() -> Result<Option<String>, String> {
    file_picker::open();
    Ok(None)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn open_now() -> Result<Option<String>, String> {
    let path = native_doc_path()?;
    std::fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| format!("no document to open ({}: {e})", path.display()))
}

/// Files chosen through the web file dialog since the last drain.
#[cfg(target_arch = "wasm32")]
pub fn take_opened() -> Vec<(String, String)> {
    file_picker::take_queue()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn take_opened() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn native_doc_path() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("executable has no directory")?;
    Ok(dir.join("shader.wesl"))
}

// --- settings ----------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub fn load_settings() -> Option<String> {
    local_storage()?.get_item(SETTINGS_KEY).ok().flatten()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_settings() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn save_settings(text: &str) {
    if let Some(ls) = local_storage() {
        let _ = ls.set_item(SETTINGS_KEY, text);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save_settings(_text: &str) {}

// --- web file dialog -----------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod file_picker {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{Blob, Event, HtmlElement, HtmlInputElement};

    /// The dialog's `onchange` closure and the input it is wired to; both
    /// must outlive `open()`, so the latest pair is parked here.
    type Dialog = (Closure<dyn FnMut(Event)>, HtmlInputElement);

    thread_local! {
        /// `(file name, text)` pairs completed since the last drain.
        static QUEUE: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        /// Keep the dialog's closure + input alive until the next dialog.
        static DIALOG: RefCell<Option<Dialog>> = RefCell::new(None);
    }

    /// Pop the browser's file picker; results land in [`take_queue`].
    pub fn open() {
        let Some(input) = make_input() else { return };
        let queue = QUEUE.with(Rc::clone);
        let input_cb = input.clone();
        let on_change = Closure::wrap(Box::new(move |_: Event| {
            let Some(file) = input_cb.files().and_then(|files| files.get(0)) else {
                return;
            };
            let name = file.name();
            // File extends Blob, so `text()` (Blob::text) is available.
            let promise = file.unchecked_ref::<Blob>().text();
            let queue = Rc::clone(&queue);
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(js) = wasm_bindgen_futures::JsFuture::from(promise).await
                    && let Some(text) = js.as_string()
                {
                    queue.borrow_mut().push((name, text));
                }
            });
        }) as Box<dyn FnMut(Event)>);
        let html: &HtmlElement = input.unchecked_ref();
        html.set_onchange(Some(on_change.as_ref().unchecked_ref()));
        // A detached input still opens the picker; no DOM pollution.
        html.click();
        DIALOG.with(|d| *d.borrow_mut() = Some((on_change, input)));
    }

    pub fn take_queue() -> Vec<(String, String)> {
        QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
    }

    fn make_input() -> Option<HtmlInputElement> {
        let document = web_sys::window()?.document()?;
        let input: HtmlInputElement = document
            .create_element("input")
            .ok()?
            .dyn_into()
            .expect("input element");
        input.set_type("file");
        input.set_accept(".wesl,.wgsl,.glsl,.txt");
        Some(input)
    }
}
