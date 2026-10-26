//! Native [`Link`]: the `wgsl-analyzer` executable over stdio.
//!
//! The server reads its workspace from disk. [`Transport::start`] hands the
//! work to a thread that writes the seed files into a per-process temp
//! directory, spawns the server and then feeds its stdin, so neither the
//! files nor a server slow to read its input ever stall a frame: messages
//! queue on a channel meanwhile. A second thread splits stdout into LSP
//! messages; the client polls them. Dropping the transport kills the
//! server, and the writer thread removes the directory on its way out.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

use bevy::log::warn;
use serde_json::Value;

use super::{Link, LinkState};

pub struct Transport {
    /// Bodies for the writer thread to frame and send.
    outgoing: Sender<String>,
    /// Behind a `Mutex` only so the resource is `Sync`; the client is the
    /// sole reader and uses `get_mut`.
    incoming: Mutex<Receiver<Value>>,
    server: Arc<Mutex<Server>>,
    root: PathBuf,
}

/// The server as the writer thread brings it up.
enum Server {
    Starting,
    Running(Child),
    Failed(String),
    /// The transport was dropped: whoever sees this stops the server.
    Dropped,
}

impl Transport {
    /// Start `$WGSL_ANALYZER` (default: `wgsl-analyzer` from `PATH`) on a
    /// fresh workspace holding `files`, in the background.
    pub fn start(files: Vec<(String, &'static str)>) -> Result<Self, String> {
        let exe = std::env::var_os("WGSL_ANALYZER").unwrap_or_else(|| "wgsl-analyzer".into());
        let root =
            std::env::temp_dir().join(format!("wesl-shadertoy-analyzer-{}", std::process::id()));
        Self::start_in(exe, root, files)
    }

    fn start_in(
        exe: std::ffi::OsString,
        root: PathBuf,
        files: Vec<(String, &'static str)>,
    ) -> Result<Self, String> {
        let (outgoing, to_send) = channel::<String>();
        let (received, incoming) = channel();
        let server = Arc::new(Mutex::new(Server::Starting));

        let (thread_root, thread_server) = (root.clone(), server.clone());
        std::thread::Builder::new()
            .name("wgsl-analyzer-writer".into())
            .spawn(move || {
                let lock = || thread_server.lock().unwrap_or_else(PoisonError::into_inner);
                let stdin = match launch(&exe, &thread_root, &files, received) {
                    Ok((child, stdin)) => {
                        let mut server = lock();
                        if let Server::Dropped = *server {
                            stop(child);
                            None
                        } else {
                            *server = Server::Running(child);
                            Some(stdin)
                        }
                    }
                    Err(why) => {
                        *lock() = Server::Failed(why);
                        None
                    }
                };
                if let Some(mut stdin) = stdin {
                    // Ends once the transport is dropped, or the server
                    // stops reading (it was killed, or it exited).
                    for body in to_send {
                        let sent = write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
                            .and_then(|()| stdin.flush());
                        if sent.is_err() {
                            break;
                        }
                    }
                }
                // Keep a failure for `state` to report.
                let mut server = lock();
                if let Server::Running(_) = *server
                    && let Server::Running(child) = std::mem::replace(&mut *server, Server::Dropped)
                {
                    drop(server);
                    stop(child);
                }
                let _ = std::fs::remove_dir_all(&thread_root);
            })
            .map_err(|e| e.to_string())?;

        Ok(Self {
            outgoing,
            incoming: Mutex::new(incoming),
            server,
            root,
        })
    }
}

/// Write the workspace, spawn the server in it and start the reader thread
/// that passes its messages to `received`. Returns the server and its stdin.
fn launch(
    exe: &std::ffi::OsStr,
    root: &Path,
    files: &[(String, &str)],
    received: Sender<Value>,
) -> Result<(Child, std::process::ChildStdin), String> {
    for (path, contents) in files {
        let path = root.join(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    let mut child = Command::new(exe)
        .current_dir(root)
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

    let reader = std::thread::Builder::new()
        .name("wgsl-analyzer-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(body) = read_message(&mut reader) {
                match serde_json::from_slice(&body) {
                    Ok(message) => {
                        if received.send(message).is_err() {
                            break; // transport dropped
                        }
                    }
                    Err(e) => warn!("wgsl-analyzer sent invalid JSON: {e}"),
                }
            }
        });
    if let Err(e) = reader {
        stop(child);
        return Err(e.to_string());
    }
    Ok((child, stdin))
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl Link for Transport {
    fn state(&mut self) -> LinkState {
        let mut server = self.server.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *server {
            Server::Starting => LinkState::Starting,
            Server::Running(child) => match child.try_wait() {
                Ok(None) => LinkState::Ready,
                Ok(Some(status)) => LinkState::Failed(format!("wgsl-analyzer exited ({status})")),
                Err(e) => LinkState::Failed(e.to_string()),
            },
            Server::Failed(why) => LinkState::Failed(why.clone()),
            Server::Dropped => LinkState::Failed("wgsl-analyzer stopped".into()),
        }
    }

    fn root_uri(&self) -> String {
        file_uri(&self.root)
    }

    fn send(&mut self, message: &Value) {
        // A dead server shows up in `state` next frame.
        let _ = self.outgoing.send(message.to_string());
    }

    fn recv(&mut self) -> Vec<Value> {
        match self.incoming.get_mut() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for Transport {
    /// Kill the server, which also frees a writer blocked on its stdin; the
    /// writer then removes the workspace.
    fn drop(&mut self) {
        let mut server = self.server.lock().unwrap_or_else(PoisonError::into_inner);
        if let Server::Running(child) = std::mem::replace(&mut *server, Server::Dropped) {
            drop(server);
            stop(child);
        }
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

    /// Messages go out through the writer thread and come back through the
    /// reader (`cat` echoes them); dropping the transport stops the server
    /// and removes the workspace.
    #[test]
    fn transport_round_trips_and_cleans_up() {
        let root = std::env::temp_dir().join(format!(
            "wesl-shadertoy-transport-test-{}",
            std::process::id()
        ));
        let files = vec![("a/b.wesl".to_string(), "fn f() {}")];
        let mut link = Transport::start_in("cat".into(), root.clone(), files).expect("starts");
        let message = serde_json::json!({ "jsonrpc": "2.0", "method": "ping" });
        link.send(&message);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut received = Vec::new();
        while received.is_empty() {
            assert!(std::time::Instant::now() < deadline, "no echo");
            received = link.recv();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(received, [message]);
        assert!(matches!(link.state(), LinkState::Ready));
        assert_eq!(
            std::fs::read_to_string(root.join("a/b.wesl")).unwrap(),
            "fn f() {}"
        );

        drop(link);
        while root.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "workspace left behind"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn missing_server_fails() {
        let root = std::env::temp_dir().join(format!(
            "wesl-shadertoy-transport-missing-{}",
            std::process::id()
        ));
        let mut link =
            Transport::start_in("no-such-wgsl-analyzer".into(), root, Vec::new()).expect("starts");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match link.state() {
                LinkState::Failed(why) => {
                    assert!(why.contains("cannot run"), "{why}");
                    break;
                }
                _ => assert!(std::time::Instant::now() < deadline, "never failed"),
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn file_uris() {
        assert_eq!(file_uri(Path::new("/tmp/a b/x")), "file:///tmp/a%20b/x");
        assert_eq!(file_uri(Path::new(r"C:\Users\x")), "file:///C:/Users/x");
    }
}
