//! Native [`Link`]: the `wgsl-analyzer` executable over stdio.
//!
//! The server reads its workspace from disk, so [`Transport::start`] writes
//! the seed files into a per-process temp directory (removed on drop). A
//! reader thread splits stdout into LSP messages; the client polls them.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

use bevy::log::warn;
use serde_json::Value;

use super::{Link, LinkState};

pub struct Transport {
    child: Child,
    stdin: ChildStdin,
    /// Behind a `Mutex` only so the resource is `Sync`; the client is the
    /// sole reader and uses `get_mut`.
    incoming: Mutex<Receiver<Value>>,
    root: PathBuf,
}

impl Transport {
    /// Spawn `$WGSL_ANALYZER` (default: `wgsl-analyzer` from `PATH`) on a
    /// fresh workspace holding `files`.
    pub fn start(files: &[(&str, &str)]) -> Result<Self, String> {
        let exe = std::env::var_os("WGSL_ANALYZER").unwrap_or_else(|| "wgsl-analyzer".into());
        let root =
            std::env::temp_dir().join(format!("wesl-shadertoy-analyzer-{}", std::process::id()));
        for (path, contents) in files {
            let path = root.join(path);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
        }

        let mut child = Command::new(&exe)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot run {}: {e}. Install wgsl-analyzer or point WGSL_ANALYZER at it.",
                    exe.to_string_lossy()
                )
            })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("wgsl-analyzer-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                while let Some(body) = read_message(&mut reader) {
                    match serde_json::from_slice(&body) {
                        Ok(message) => {
                            if tx.send(message).is_err() {
                                break; // transport dropped
                            }
                        }
                        Err(e) => warn!("wgsl-analyzer sent invalid JSON: {e}"),
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Self {
            child,
            stdin,
            incoming: Mutex::new(rx),
            root,
        })
    }
}

impl Link for Transport {
    fn state(&mut self) -> LinkState {
        match self.child.try_wait() {
            Ok(None) => LinkState::Ready,
            Ok(Some(status)) => LinkState::Failed(format!("wgsl-analyzer exited ({status})")),
            Err(e) => LinkState::Failed(e.to_string()),
        }
    }

    fn root_uri(&self) -> String {
        file_uri(&self.root)
    }

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        // A dead server shows up in `state` next frame.
        let _ = write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .and_then(|()| self.stdin.flush());
    }

    fn recv(&mut self) -> Vec<Value> {
        match self.incoming.get_mut() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One LSP message body (`Content-Length` framing), or `None` at EOF or on
/// a malformed header.
fn read_message(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

/// `file://` URI of an absolute path (`file:///C:/...` on Windows).
fn file_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from(if path.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let body = r#"{"jsonrpc":"2.0","method":"x","params":"é"}"#;
        let wire = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}\
             content-length: 2\r\n\r\n{{}}",
            body.len()
        );
        let mut reader = BufReader::new(wire.as_bytes());
        assert_eq!(read_message(&mut reader).unwrap(), body.as_bytes());
        assert_eq!(read_message(&mut reader).unwrap(), b"{}");
        assert_eq!(read_message(&mut reader), None);
    }

    #[test]
    fn file_uris() {
        assert_eq!(file_uri(Path::new("/tmp/a b/x")), "file:///tmp/a%20b/x");
        assert_eq!(file_uri(Path::new(r"C:\Users\x")), "file:///C:/Users/x");
    }
}
