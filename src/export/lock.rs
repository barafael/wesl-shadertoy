//! Export → lock screen: the document as a standalone lock screen for niri
//! and the other compositors with `ext-session-lock-v1` (sway, Hyprland).
//!
//! The crate is plain wgpu on smithay-client-toolkit, no Bevy: `locker/`
//! holds its program, the same for every export, and [`config_rs`] writes
//! the part that differs: the title, the frame-rate cap and the constants'
//! values (baked in, since nothing on a lock screen edits them). Passwords
//! go to PAM.

use super::{Names, lygia_note, rust_str};
use crate::tweak::{Constant, Kind, MAX_TWEAKS};

/// The files every lock screen shares.
pub(super) const MAIN_RS: &str = include_str!("locker/main.rs");
pub(super) const VERTEX_WGSL: &str = include_str!("locker/vertex.wgsl");
pub(super) const INDICATOR_WGSL: &str = include_str!("locker/indicator.wgsl");

/// The crate's files for the linked `wgsl` of `source`, capped at `fps`
/// frames per second (0: the display's rate).
pub(super) fn files(
    names: &Names,
    title: &str,
    source: &str,
    wgsl: String,
    tweaks: &[Constant],
    fps: u32,
) -> Vec<(String, String)> {
    vec![
        ("Cargo.toml".to_string(), cargo_toml(names, title)),
        ("src/main.rs".into(), MAIN_RS.into()),
        ("src/config.rs".into(), config_rs(title, tweaks, fps)),
        ("src/shader.wgsl".into(), wgsl),
        ("src/vertex.wgsl".into(), VERTEX_WGSL.into()),
        ("src/indicator.wgsl".into(), INDICATOR_WGSL.into()),
        ("src/shader.wesl".into(), source.to_string()),
        ("README.md".into(), readme(names, title, source, fps)),
        (".gitignore".into(), "/target\n".into()),
    ]
}

fn cargo_toml(names: &Names, title: &str) -> String {
    let description = format!("{title}, exported from wesl-shadertoy as a lock screen.");
    format!(
        r#"[package]
name = "{dir}"
version = "0.1.0"
edition = "2024"
description = {description}
publish = false

[dependencies]
wgpu = "30"
smithay-client-toolkit = "0.20"
# `system`: libwayland underneath, whose pointers wgpu needs.
wayland-client = {{ version = "0.31", features = ["system"] }}
raw-window-handle = "0.6"
pollster = "0.4"
# Without its default `cli` feature, which reads passwords from a terminal.
pam-client = {{ version = "0.5", default-features = false }}
zeroize = "1"
# PAM's messages, drawn in the system's font.
ab_glyph = "0.2"
libc = "0.2"
# `iDate` in local time, as on shadertoy.
jiff = "0.2"

[profile.release]
# A panic must not unwind through PAM or Wayland's C code; the compositor
# keeps the session locked whatever happens to this process.
panic = "abort"

# A workspace of its own, so the crate builds wherever it is unpacked.
[workspace]
"#,
        dir = names.dir,
        description = rust_str(&description),
    )
}

/// `src/config.rs`: the title, the cap and the `i_tweaks` slots.
fn config_rs(title: &str, tweaks: &[Constant], fps: u32) -> String {
    let mut slots = String::new();
    for slot in 0..MAX_TWEAKS {
        let Some(c) = tweaks.get(slot) else {
            slots.push_str("    [0.0; 4],\n");
            continue;
        };
        let used = match c.kind {
            Kind::Vec(n) => n as usize,
            _ => 1,
        };
        let values: Vec<String> = (0..4)
            .map(|i| format!("{:?}", if i < used { c.value[i] } else { 0.0 }))
            .collect();
        slots.push_str(&format!("    [{}], // {}\n", values.join(", "), c.name));
    }
    format!(
        r#"//! What this lock screen was exported with from wesl-shadertoy.

/// The shader's name.
pub const TITLE: &str = {title};

/// Frames per second at most, unless `--fps` says otherwise; 0 draws at the
/// display's rate.
pub const DEFAULT_FPS: u32 = {fps};

/// The shader's constants, one `i_tweaks` slot each, as they were set in
/// the editor. Edit a value here to change it.
pub const TWEAKS: [[f32; 4]; {MAX_TWEAKS}] = [
{slots}];
"#,
        title = rust_str(title),
    )
}

fn readme(names: &Names, title: &str, source: &str, fps: u32) -> String {
    let dir = &names.dir;
    let cap = if fps == 0 {
        "at the display's rate".to_string()
    } else {
        format!("at most {fps} frames per second")
    };
    let lygia = lygia_note(source);
    format!(
        r#"# {dir}

{title}, exported from [wesl-shadertoy](https://github.com/barafael/wesl-shadertoy)
as a lock screen for [niri](https://github.com/YaLTeR/niri) and the other
Wayland compositors with `ext-session-lock-v1` (sway, Hyprland; not KDE or
GNOME).

It locks the session and draws the shader on every output, {cap}. Type
the password and press Enter; dots show what was typed, Escape or Ctrl+U
clears it, and an orange bar warns about Caps Lock. What PAM says, such as
an account locked after too many failed attempts, shows under the dots in
the system's sans-serif font (found with fontconfig's `fc-match`).

## Install

Needs Rust, and the development files of PAM, xkbcommon and Wayland
(`pam`, `libxkbcommon` and `wayland` on Arch; `libpam0g-dev`,
`libxkbcommon-dev` and `libwayland-dev` on Debian and Ubuntu).

```sh
cargo install --path .
```

Then bind it in niri's `config.kdl` (niri does not read `PATH` from your
shell profile, so give the full path):

```kdl
binds {{
    Mod+Alt+L {{ spawn "/home/you/.cargo/bin/{dir}"; }}
}}
```

or lock when idle with [swayidle](https://github.com/swaywm/swayidle):

```sh
swayidle -w timeout 300 {dir}
```

## Options

- `--fps N`: frames per second at most; `0` draws at the display's rate.
  The default is in `src/config.rs`. The GPU runs the shader for as long
  as the screen is locked and on, so a lower cap saves power.
- `--pam-service NAME`: the PAM service that checks the password; the
  default, `login`, is what swaylock includes too. For a service of its
  own, create `/etc/pam.d/{dir}` holding `auth include login` and pass
  `--pam-service {dir}`.

## If it fails

The compositor keeps the session locked, not this program: if it crashes
or the GPU resets, the screen stays locked and blank, and only a locker
that checks your password unlocks it. Try it out once with a second
locker such as swaylock installed, which can take over the locked session
from a TTY (Ctrl+Alt+F2, log in, then
`WAYLAND_DISPLAY=wayland-1 swaylock`, with the display niri reports).

There is no daemon mode yet, so a `before-sleep` hook cannot wait for the
lock to be in place before the machine suspends.

## The shader

`src/shader.wgsl` is the linked WGSL; `src/shader.wesl` is the source it
was compiled from, for reference (wesl-shadertoy's dialect, not built
here). Its constants are baked into `src/config.rs`. `iMouse` stays zero.

## License

See the header of `src/shader.wesl` for the shader's author and license.{lygia}
"#
    )
}
