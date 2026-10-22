# wesl-shadertoy

A classic [shadertoy](https://www.shadertoy.com)-style live shader playground for
[WESL](https://github.com/adanoside/wesl), built on **Bevy 0.20 + egui**.
One animated fullscreen fragment shader, one code editor, `▶ Run`.
Runs natively and in the browser as WASM.

> **Live demo:** <https://barafael.github.io/wesl-shadertoy/>

## Controls

| What | How |
| --- | --- |
| Compile & apply | `▶ Run` button or **Ctrl/Cmd+Enter** |
| Pause / resume the clock, reset, speed | toolbar |
| Presets | `preset:` combobox (Plasma / Raymarch / Mandelbrot) |
| Windows | `Code`, `WGSL` (composed output), `Session` toggles |
| Edit | Tab / Shift-Tab indent, line-number gutter, syntax highlighting |
| `iMouse` | click + drag on the canvas (clicks on UI are ignored) |

The document auto-saves to `localStorage` (browser builds) and is restored on
load. Compile errors appear in the bottom panel — the last good shader keeps
running while you fix them.

## The shader dialect

Your editor text is the virtual WESL module `package::user`, compiled verbatim
at runtime (so error line numbers match the editor). Two hidden modules are
added around it:

* `package::shadertoy` — the uniform block plus shadertoy-style accessors.
* `package::main` — imports your `main_image` and defines the `@fragment`
  entry point.

Your entry point returns the color instead of writing to an out variable:

```rust
import package::shadertoy::{i_time, i_resolution};

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = frag_coord / res;
    let t = i_time();
    let col = 0.5 + 0.5 * cos(t + uv.xyx + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
```

Available uniforms (import from `package::shadertoy`):

| Function | Shadertoy equivalent | Meaning |
| --- | --- | --- |
| `i_time() -> f32` | `iTime` | seconds since (re)start |
| `i_time_delta() -> f32` | `iTimeDelta` | seconds since previous frame |
| `i_frame() -> u32` | `iFrame` | frame counter |
| `i_resolution() -> vec3<f32>` | `iResolution` | viewport size in pixels |
| `i_mouse() -> vec4<f32>` | `iMouse` | xy: drag position, zw: click; negative z after release |
| `i_date() -> vec4<f32>` | `iDate` | seconds since midnight, month, day of week, day of year |

`frag_coord` is in pixels, origin **top-left** (WGSL framebuffer convention,
like shadertoy's `gl_FragCoord`).

The pipeline: the editor text is linked by the WESL compiler, the composed
WGSL is parsed and validated with `naga` (same major version as the wgpu
inside Bevy 0.20, so type errors surface in the UI instead of the browser
console), then handed to Bevy as `Shader::from_wgsl`. A new shader handle per
successful compile swaps into the render pass atomically — a broken edit never
blanks the screen.

## Architecture

```
src/
  main.rs       app wiring, resources, per-frame uniforms, egui UI, persistence
  compile.rs    WESL link + naga validate (the shader contract above)
  render/
    mod.rs                    ShaderToyUniform/Fragment components + plugin
    fullscreen_material.rs    vendored from Bevy 0.20, modified to rebuild
                              its pipeline when the shader handle changes
  highlight.rs  WESL/WGSL tokenizer -> colored LayoutJob
  editor.rs     the code editor: highlighted galley behind a transparent
                TextEdit, line-number gutter, Tab/Shift-Tab
  presets.rs    Plasma / Raymarch / Mandelbrot (compile-checked in tests)
  net.rs        multiplayer seams (see below)
```

Rendering is the vendored Bevy `fullscreen_material` plugin: the user shader
runs as a fullscreen pass in `Core3d`'s post-process stage with
`Tonemapping::None`, so colors hit the screen untouched. Per-frame uniforms
ride in a dynamic uniform buffer bound at `@group(0) @binding(2)`.

The UI is egui (via `bevy_egui`, `EguiPrimaryContextPass`), following the
integration pattern proven in [omdurman](https://github.com/barafael/omdurman).

## Multiplayer roadmap (already wired in)

`src/net.rs` ships the seams, not the network:

* `SessionMessage` — serde wire format: `Join` / `Leave` / `ShaderEdit`
  (small `EditIntent`s: base version + replace range + insertion) /
  `ShaderFull` snapshots.
* `SyncTransport` — transport seam with a `LoopbackTransport` for solo use.
  The planned transport is **matchbox** WebRTC data channels + signaling,
  exactly like omdurman ships.
* Stale edits are rejected by version and fall back to a full snapshot; the
  upgrade path for true concurrent editing is a CRDT (`yrs`/`automerge`)
  behind the same `ShaderEdit` payload.

The editor already emits `EditIntent`s on every change, so a session can be
added without touching the editor or the renderer.

## Development

```sh
cargo run              # native (dynamic Bevy, fast rebuilds)
cargo test             # compile pipeline, presets, net, highlighter
trunk serve            # browser build on http://127.0.0.1:8080
```

Pinned toolchain: see `rust-toolchain.toml` (stable + `wasm32-unknown-unknown`).
Release wasm is built with `trunk build --release` (trunk ≥ 0.21 for the
`data-wasm-opt-params` flag — Rust emits bulk-memory ops that trunk's bundled
wasm-opt needs told about).

GitHub Pages deployment lives in `.github/workflows/deploy.yml` (trunk v0.22.0,
`--public-url /<repo>/`).

## License

MIT OR Apache-2.0. `src/render/fullscreen_material.rs` is vendored (and
modified) from Bevy 0.20 under the same dual license — see its header.
