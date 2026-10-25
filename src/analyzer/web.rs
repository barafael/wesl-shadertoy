//! Web [`Link`]: wgsl-analyzer-web in a Web Worker, through `web/analyzer.js`.
//!
//! The JS module owns the server; this side only polls it (no callbacks
//! into Rust), the same queue pattern as the file dialog in
//! [`crate::persist`].

use serde_json::Value;
use wasm_bindgen::prelude::wasm_bindgen;

use super::{Link, LinkState};

#[wasm_bindgen(module = "/web/analyzer.js")]
extern "C" {
    fn analyzer_start(root: &str, files_json: &str);
    fn analyzer_stop();
    fn analyzer_state() -> String;
    fn analyzer_send(json: &str);
    fn analyzer_drain() -> js_sys::Array;
}

/// Workspace root in the worker's in-memory filesystem.
const ROOT: &str = "/workspace";

/// The server itself lives in the JS module; at most one at a time.
pub struct Transport;

impl Transport {
    /// Begin loading the server; [`Link::state`] reports when it is up.
    pub fn start(files: &[(String, &str)]) -> Result<Self, String> {
        let files: serde_json::Map<String, Value> = files
            .iter()
            .map(|(path, contents)| (path.clone(), Value::from(*contents)))
            .collect();
        analyzer_start(ROOT, &Value::Object(files).to_string());
        Ok(Self)
    }
}

impl Link for Transport {
    fn state(&mut self) -> LinkState {
        let state = analyzer_state();
        match state.as_str() {
            "ready" => LinkState::Ready,
            "idle" | "loading" => LinkState::Starting,
            failed => LinkState::Failed(failed.trim_start_matches("failed: ").to_owned()),
        }
    }

    fn root_uri(&self) -> String {
        format!("file://{ROOT}")
    }

    fn send(&mut self, message: &Value) {
        analyzer_send(&message.to_string());
    }

    fn recv(&mut self) -> Vec<Value> {
        analyzer_drain()
            .iter()
            .filter_map(|m| m.as_string())
            .filter_map(|m| serde_json::from_str(&m).ok())
            .collect()
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        analyzer_stop();
    }
}
