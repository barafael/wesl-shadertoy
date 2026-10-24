# wesl-shadertoy

A classic [shadertoy](https://www.shadertoy.com)-style live shader playground for
[WESL](https://github.com/adanoside/wesl), built on **Bevy 0.20 + egui**.
One animated fullscreen fragment shader, one code editor, one compile button.
Runs natively and in the browser as WASM.

> **Live demo:** <https://barafael.github.io/wesl-shadertoy/>

## Controls

| What | How |
| --- | --- |
| Compile & apply | **Compile WESL Shader** button or **Ctrl/Cmd+Enter** |
| Save / open | **Ctrl/Cmd+S** (browser storage, or `shader.wesl` natively) · **Ctrl/Cmd+O** (file picker / `shader.wesl`) |
| Pause / resume the clock, reset, speed | toolbar |
| Presets | `preset:` combobox — 12 shadertoy classics (Plasma, Star Nest, Menger, …); fresh sessions roll a random one |
| Windows | `Code`, `WGSL` (composed output) toggles; `⚙` settings (Gruvbox dark/light), `?` about |
| Edit | Tab / Shift-Tab indent, line-number gutter, syntax highlighting |
| Live diagnostics | squiggles + gutter marks from [wgsl-analyzer](https://github.com/wgsl-analyzer/wgsl-analyzer) while you type; hover for the message; toggle in `⚙` |
| `iMouse` | click + drag on the canvas (clicks on UI are ignored) |
| Debugger overlay | toolbar bug icon (or `--debugger` natively): crosshair and click ring, RGBA probe boxes for the pixels under the cursor and the last click, mouse position, resolution, fps |

The document auto-saves to `localStorage` (browser builds) and is restored on
load, along with your theme. Compile errors appear in the bottom panel — the
last good shader keeps running while you fix them.

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
| `i_frame_rate() -> f32` | `iFrameRate` | smoothed frames per second |
| `i_date() -> vec4<f32>` | `iDate` | seconds since midnight, month, day of week, day of year |

`frag_coord` is in pixels, origin **top-left** (WGSL framebuffer convention,
like shadertoy's `gl_FragCoord`).

One dialect gotcha when porting snippets: `mod` is a reserved word in WESL (it
declares a module), so write the builtin out as `x - y * floor(x / y)` instead
of calling `mod(x, y)`.

The pipeline: the editor text is linked by the WESL compiler, the composed
WGSL is parsed and validated with `naga` (same major version as the wgpu
inside Bevy 0.20, so type errors surface in the UI instead of the browser
console), then handed to Bevy as `Shader::from_wgsl`. A new shader handle per
successful compile swaps into the render pass atomically — a broken edit never
blanks the screen.

## Live diagnostics (wgsl-analyzer)

While you edit, the [wgsl-analyzer](https://github.com/wgsl-analyzer/wgsl-analyzer)
language server checks the code and the editor squiggles what it reports;
the status bar shows the count. Its workspace mirrors the real compile:
`shaders/shadertoy.wesl` is the hidden `package::shadertoy` module and the
editor text is `shaders/user.wesl` (`package::user`), so imports and types
resolve exactly as when compiling.

* **Browser:** the server runs in a Web Worker from
  [`wgsl-analyzer-web`](https://www.npmjs.com/package/wgsl-analyzer-web)
  (the same server compiled to wasm). `web/fetch-wgsl-analyzer.sh` fetches
  the pinned version (integrity-checked) as a trunk `pre_build` hook, and
  `index.html` copies it to `dist/wgsl-analyzer/`. The worker needs
  `SharedArrayBuffer`, i.e. a cross-origin isolated page: `trunk serve` sends
  the COOP/COEP headers (`Trunk.toml`); on hosts that cannot (GitHub Pages)
  the vendored [`coi-serviceworker`](https://github.com/gzuidhof/coi-serviceworker)
  adds them, reloading once on the first visit.
* **Native:** the app runs the `wgsl-analyzer` executable over stdio —
  `$WGSL_ANALYZER` if set, else from `PATH`. Build it from a clone of the
  repository with `cargo build --release -p wgsl-analyzer`. Without it the
  status bar reads "wgsl-analyzer: unavailable" and everything else works.

## Architecture

```
src/
  main.rs       app wiring, resources, per-frame uniforms, egui UI
  compile.rs    WESL link + naga validate (the shader contract above)
  persist.rs    document/settings IO: localStorage, shader.wesl, file picker
  theme.rs      Gruvbox egui Visuals + per-theme syntax palettes (dark/light)
  render/
    mod.rs                    ShaderToyUniform/Fragment components + plugin
    fullscreen_material.rs    vendored from Bevy 0.20, modified to rebuild
                              its pipeline when the shader handle changes
  debugger.rs   debugger overlay (ported from bevy_shadertoy_wgsl): blit pass,
                overlay WGSL, font atlas rasterized from egui's Hack font
  highlight.rs  WESL/WGSL tokenizer -> colored LayoutJob
  editor.rs     the code editor: TextEdit with a custom layouter that paints
                the highlighted job, line-number gutter, Tab/Shift-Tab
  presets.rs    12 compile-checked shadertoy classic presets
  analyzer/     wgsl-analyzer LSP client (live diagnostics)
    mod.rs        handshake, full-text sync, publishDiagnostics -> char ranges
    native.rs     the wgsl-analyzer executable over stdio
    web.rs        wgsl-analyzer-web in a Web Worker, via web/analyzer.js
web/
  analyzer.js               JS side of the web transport
  fetch-wgsl-analyzer.sh    fetches the pinned wgsl-analyzer-web build
  coi-serviceworker.js      COOP/COEP for hosts without header control (MIT)
```

Rendering is the vendored Bevy `fullscreen_material` plugin: the user shader
runs as a fullscreen pass in `Core3d`'s post-process stage with
`Tonemapping::None`, so colors hit the screen untouched. Per-frame uniforms
ride in a dynamic uniform buffer bound at `@group(0) @binding(2)`. While the
debugger overlay is on, the user shader renders into an offscreen
`Rgba16Float` target instead (`Rgba8UnormSrgb` where WebGL2 cannot render
to half floats), and a second pass draws it plus the overlay to the view;
while it is off, that pass and the target cost nothing.

The UI is egui (via `bevy_egui`, `EguiPrimaryContextPass`), following the
integration pattern proven in [omdurman](https://github.com/barafael/omdurman).

## Development

```sh
cargo run              # native (dynamic Bevy, fast rebuilds)
cargo test             # compile pipeline, presets, highlighter, editor
cargo run -- --render Plasma out.png [--debugger]  # headless PNG, no window
WGSL_ANALYZER=/path/to/wgsl-analyzer cargo test -- --ignored  # + live server
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
modified) from Bevy 0.20 under the same dual license — see its header. The
debugger overlay in `src/debugger.rs` is ported from
[eliotbo/bevy_shadertoy_wgsl](https://github.com/eliotbo/bevy_shadertoy_wgsl)
(MIT OR Apache-2.0).
