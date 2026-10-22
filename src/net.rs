//! Multiplayer groundwork for wesl-shadertoy.
//!
//! This module deliberately ships *no actual networking* yet. It defines the
//! seams a real session will plug into, so the rest of the app is written
//! against them from day one:
//!
//! * [`SessionMessage`] — the wire format (serde + JSON today; any binary
//!   codec later). Edits travel as small [`EditIntent`]s (base version +
//!   replace-range + insertion), full documents as `ShaderFull` snapshots.
//! * [`SyncTransport`] — the transport seam. [`LoopbackTransport`] is the
//!   single-user implementation (sends go nowhere, nothing arrives). The
//!   planned upgrade is [`matchbox_socket`](https://github.com/johanhoffmann/matchbox)
//!   WebRTC data channels with a signaling server, exactly like the
//!   `omdurman` project ships for its turn-based game.
//! * [`SessionState`] — who is in the session and the transport status.
//!
//! ## Conflict handling roadmap
//!
//! The document is the source of truth and carries a monotonically increasing
//! `version`. An [`EditIntent`] names the `base_version` it was produced
//! against; if that is stale the receiver falls back to requesting/accepting a
//! `ShaderFull`. That is enough for a two-person "follow the leader" session.
//! For true concurrent editing the next step is swapping the plain `String`
//! document for a CRDT ([`yrs`](https://github.com/y-crdt/y-crdt) or
//! `automerge`); the `ShaderEdit` payload then becomes an opaque CRDT update
//! and `apply_edit` becomes a merge. Nothing else in the app has to change.

#![allow(dead_code, reason = "multiplayer seams: wired up when a real transport lands")]

use std::collections::VecDeque;
use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Identifies a participant in a shader session.
pub type PeerId = u64;

/// A minimal edit intent: in a document at `base_version`, replace the
/// character range `range` with `insert`. Produced by [`diff_edit`] and
/// applied by [`apply_edit`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditIntent {
    pub base_version: u64,
    pub range: Range<usize>,
    pub insert: String,
}

/// Everything that can cross the wire between session participants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SessionMessage {
    /// A peer joined and is announcing its document (host answers with `ShaderFull`).
    Join { peer: PeerId, name: String },
    /// A peer left.
    Leave { peer: PeerId },
    /// An incremental edit against a known document version.
    ShaderEdit {
        from: PeerId,
        base_version: u64,
        range: Range<usize>,
        insert: String,
    },
    /// A complete document; authoritative, bumps the local version.
    ShaderFull {
        from: PeerId,
        version: u64,
        text: String,
    },
}

/// Transport status surfaced in the session UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransportStatus {
    /// Single-user mode: edits never leave the machine.
    #[default]
    LocalOnly,
    /// A real transport is connecting.
    Connecting,
    /// Connected to a session.
    Connected,
}

/// The transport seam. Implementations move [`SessionMessage`]s between peers.
pub trait SyncTransport: Send {
    /// Queue a locally-generated message for the session.
    fn send(&mut self, msg: SessionMessage);
    /// Drain one received message, if any.
    fn poll(&mut self) -> Option<SessionMessage>;
    /// Current status.
    fn status(&self) -> TransportStatus;
}

/// The single-user transport: `send` drops into an internal queue that only
/// tests (or a future "echo a peer" demo mode) read back.
#[derive(Default)]
pub struct LoopbackTransport {
    queue: VecDeque<SessionMessage>,
}

impl SyncTransport for LoopbackTransport {
    fn send(&mut self, msg: SessionMessage) {
        // Single-user: keep it observable for tests/debug, deliver to nobody.
        self.queue.push_back(msg);
    }
    fn poll(&mut self) -> Option<SessionMessage> {
        // Nothing arrives in local-only mode. (A future matchbox transport
        // drains its WebRTC channel here.)
        None
    }
    fn status(&self) -> TransportStatus {
        TransportStatus::LocalOnly
    }
}

impl LoopbackTransport {
    /// Peek what the local user has "sent" (debug/tests).
    pub fn sent(&self) -> Option<&SessionMessage> {
        self.queue.front()
    }
}

/// Bevy resource wrapping the active transport. Swapping the single-user
/// `LoopbackTransport` for a matchbox WebRTC transport is a one-line change
/// here plus a connection state machine.
#[derive(bevy::ecs::resource::Resource, Default)]
pub struct NetTransport {
    pub inner: LoopbackTransport,
}

impl NetTransport {
    pub fn send(&mut self, msg: SessionMessage) {
        self.inner.send(msg);
    }
    pub fn poll(&mut self) -> Option<SessionMessage> {
        self.inner.poll()
    }
}

/// Compute a single-edit diff between two documents using the classic common
/// prefix/suffix trim. Returns `None` when the change is not expressible as
/// one contiguous replace (e.g. two distant edits in one frame), in which case
/// callers should fall back to a `ShaderFull` snapshot.
pub fn diff_edit(base_version: u64, old: &str, new: &str) -> Option<EditIntent> {
    if old == new {
        return None;
    }
    let oldb = old.as_bytes();
    let newb = new.as_bytes();

    // Common prefix (byte-wise is fine: both sides are the same text up to the
    // first difference, and UTF-8 boundaries cannot split inside equal bytes).
    let prefix = oldb.iter().zip(newb).take_while(|(a, b)| a == b).count();
    // Common suffix, not overlapping the prefix.
    let suffix = oldb[prefix..]
        .iter()
        .rev()
        .zip(newb[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let range = prefix..old.len() - suffix;
    let insert = &new[prefix..new.len() - suffix];
    Some(EditIntent {
        base_version,
        range: range.clone(),
        insert: insert.to_string(),
    })
}

/// Apply an edit intent to `text`, which is currently at `version`.
/// Fails (returns `false`, changes nothing) when the intent names a different
/// `base_version` than the document has — the caller then falls back to a
/// `ShaderFull` snapshot. Also rejects ranges that are not valid character
/// boundaries in `text` (a sign the documents diverged).
pub fn apply_edit(text: &mut String, version: u64, intent: &EditIntent) -> bool {
    if intent.base_version != version {
        return false;
    }
    let start = intent.range.start;
    let end = intent.range.end;
    if start > end || end > text.len() {
        return false;
    }
    // Never split a UTF-8 codepoint.
    if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return false;
    }
    text.replace_range(start..end, &intent.insert);
    true
}

/// Who is in the session and how the transport is doing.
#[derive(bevy::ecs::resource::Resource, Default)]
pub struct SessionState {
    pub local_peer: PeerId,
    pub peers: Vec<(PeerId, String)>,
    pub status: TransportStatus,
}
impl SessionState {
    pub fn new_local() -> Self {
        Self {
            // Random-ish local id from the wall clock's fractional second;
            // a real session negotiates these.
            local_peer: ((crate::now_epoch_secs().fract() * 1_000_000.0) as u64 & 0xfffe) | 1,
            peers: Vec::new(),
            status: TransportStatus::LocalOnly,
        }
    }
}

/// Bevy plugin registering the session state. Later this will also drive the
/// matchbox connection state machine.
pub struct NetPlugin;

impl bevy::app::Plugin for NetPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<SessionState>().init_resource::<NetTransport>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_edit() {
        let old = "fn main_image() -> vec4<f32> { return vec4(0.0); }";
        let new = "fn main_image() -> vec4<f32> { return vec4(1.0, 0.5, 0.2, 1.0); }";
        let intent = diff_edit(7, old, new).expect("single contiguous edit");
        assert_eq!(intent.base_version, 7);
        let mut text = old.to_string();
        assert!(apply_edit(&mut text, 7, &intent));
        assert_eq!(text, new);
    }

    #[test]
    fn insertion_at_end() {
        let mut text = "abc".to_string();
        let intent = diff_edit(0, "abc", "abcdef").unwrap();
        assert!(apply_edit(&mut text, 0, &intent));
        assert_eq!(text, "abcdef");
    }

    #[test]
    fn stale_edit_is_rejected() {
        let intent = diff_edit(0, "abc", "abd").unwrap();
        // Receiver is already at version 1: the intent against v0 is stale.
        let mut other = "zzz".to_string();
        assert!(!apply_edit(&mut other, 1, &intent));
        assert_eq!(other, "zzz");
    }

    #[test]
    fn unicode_safe() {
        let old = "let c = 'é'; // café";
        let new = "let c = 'é'; // café ☕";
        let intent = diff_edit(0, old, new).unwrap();
        let mut text = old.to_string();
        assert!(apply_edit(&mut text, 0, &intent));
        assert_eq!(text, new);
    }

    #[test]
    fn json_roundtrip() {
        let msg = SessionMessage::ShaderEdit {
            from: 3,
            base_version: 12,
            range: 4..9,
            insert: "vec3".into(),
        };
        let s = serde_json::to_string(&msg).unwrap();
        let back: SessionMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(msg, back);
    }
}
