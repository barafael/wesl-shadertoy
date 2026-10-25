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
| Clock speed, pause, reset | toolbar speed slider (0 pauses) and reset button (reset also restarts `i_frame` and clears the buffers) |
| Examples | `Examples` window — 33 examples (Plasma, Seascape, Fluid, a dozen LYGIA demos, …) as live thumbnails; search or filter by tag, hover to play with one (the pointer drives its `iMouse`), click to open it. Fresh sessions roll a random one |
| Tweak constants | `Constants` window: a slider or color picker per literal module-level constant, live while you drag (see [Tweakable constants](#tweakable-constants)) |
| Windows | `Examples`, `Code`, `WGSL` (composed output), `Constants` toggles; `⚙` settings (frosted-glass blur slider, 0–24 px: how blurred the shader shows through the UI, 0 = off), `?` about |
| Edit | Tab / Shift-Tab indent, line-number gutter, syntax highlighting |
| Live diagnostics | squiggles + gutter marks from [wgsl-analyzer](https://github.com/wgsl-analyzer/wgsl-analyzer) while you type; hover for the message; toggle in `⚙` |
| `iMouse` | click + drag on the canvas (clicks on UI are ignored) |
| Debugger overlay | toolbar bug icon (or `--debugger` natively): crosshair and click ring, RGBA probe boxes for the pixels under the cursor and the last click, a swatch of the clicked pixel's color, mouse position, resolution, fps |

The document auto-saves to `localStorage` (browser builds) on every change —
edits, opened examples and files — and is restored on load, along with your
settings. Compile errors appear in the bottom panel — the last good shader keeps
running while you fix them, also when the GPU driver rejects a shader that
passed validation.

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

### Tweakable constants

Every module-level `const` whose value is a literal — a number or a `vecN` of
numbers — gets a control in the **Constants** window. Dragging updates the
running shader through a uniform, without recompiling, so buffer simulations
keep their state; the new value is written into your code when you let go.
A comment after the constant fine-tunes it:

```rust
const SEA_HEIGHT: f32 = 0.6;                        // slider over 0..1.2 (twice the value)
const STEPS: i32 = 8; // tweak: 1..32               // slider over 1..32
const WATER: vec3<f32> = vec3<f32>(0.5, 0.5, 0.4); // tweak: color
const PI: f32 = 3.14159; // tweak: off             // left out of the window
```

Give integer constants a type (`const N: i32 = 8;`): an untyped `8` could be
an `i32`, `u32` or `f32` at each use. Names containing `color`/`col`/`tint`
get a color picker on their own. Constants WGSL needs as true constants —
array sizes, other constants' values, `switch` cases — stay fixed, and the
window lists them. Up to 32 per shader.

Available uniforms (import from `package::shadertoy`):

| Function | Shadertoy equivalent | Meaning |
| --- | --- | --- |
| `i_time() -> f32` | `iTime` | seconds since start or Reset (scaled by the speed slider, frozen while paused) |
| `i_time_delta() -> f32` | `iTimeDelta` | `i_time` advance since the previous frame (0 while paused) |
| `i_frame() -> u32` | `iFrame` | frames since this shader started: 0 on its first frame, restarts on compile and Reset, holds while paused |
| `i_resolution() -> vec3<f32>` | `iResolution` | viewport size in pixels (z = 1) |
| `i_mouse() -> vec4<f32>` | `iMouse` | xy: drag position, zw: click; negative z after release |
| `i_frame_rate() -> f32` | `iFrameRate` | smoothed frames per second |
| `i_date() -> vec4<f32>` | `iDate` | year, month (1–12), day of month, seconds since midnight (UTC) |
| `i_sample_rate() -> f32` | `iSampleRate` | 44100 (there is no audio input) |
| `i_channel_time(i: u32) -> f32` | `iChannelTime[i]` | seconds since buffer channel `i` started (0 if the shader has no such buffer) |
| `i_channel_resolution(i: u32) -> vec3<f32>` | `iChannelResolution[i]` | size of buffer channel `i` in pixels (0 if unused) |

`frag_coord` is in pixels with the origin **bottom-left**, like shadertoy's
`fragCoord` (and `i_mouse()`); the wrapper flips WGSL's top-left
`@builtin(position)` for you.

### Buffers (multipass)

Define any of `buffer_a` … `buffer_d` next to `main_image`, with the same
signature, and each becomes a pass rendered before the image every frame, into
an `Rgba32Float` target the size of the viewport. Read the results through the
`i_channel_0` … `i_channel_3` globals (buffer A … D):

```rust
import package::shadertoy::{i_channel_0, i_frame};

fn buffer_a(frag_coord: vec2<f32>) -> vec4<f32> {
    let last = textureLoad(i_channel_0, vec2<i32>(frag_coord), 0); // previous frame
    if (i_frame() == 0u) { return vec4<f32>(0.0); }                 // first frame: init
    return last * 0.99;
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    return textureLoad(i_channel_0, vec2<i32>(frag_coord), 0);
}
```

A buffer reading itself sees its previous frame; reading an earlier buffer
sees that buffer's current frame. Buffers are unfilterable float textures:
read them with `textureLoad`, or nearest-sample them through the
`i_sampler` global. They start out black and are cleared on every compile,
on Reset and on resize.

### LYGIA

[LYGIA](https://lygia.xyz), Patricio Gonzalez Vivo's shader library, ships with
the app as the WESL package `lygia`: noise, SDFs, color spaces and palettes,
easing curves, lighting helpers and more, about 380 modules. Import what you
need by path; only what you use ends up in the shader:

```rust
import package::shadertoy::i_time;
import lygia::generative::snoise::snoise3;
import lygia::color::palette::spectral::spectral;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let n = snoise3(vec3<f32>(frag_coord / 200.0, i_time()));
    return vec4<f32>(spectral(0.5 + 0.5 * n), 1.0);
}
```

Function names carry their overload as a suffix (`snoise2`, `snoise3`, …);
browse the modules in [`vendor/lygia`](vendor/lygia) or on
[lygia.xyz](https://lygia.xyz). LYGIA's 2D shapes are centered on (0.5, 0.5),
and a few (like `heartSDF`) assume y points down, unlike `frag_coord`. Some
tonemappers (`tonemapFilmic3`, `tonemapUnreal3`) bake in gamma 2.2, which the
sRGB output then applies a second time. Its `lighting::raymarch` helpers are
still stubs in the WESL port, so bring your own marcher. The twelve
`Lygia …` examples show it in use, from noise, palettes, tilings and easing
to marbling (`distort::marble` across two buffers), post-processing filters
over a buffer, and a raymarched Cook-Torrance material study. The editor's
wgsl-analyzer still rejects `filter` (a WGSL reserved word) as a module name,
which WESL allows, so it is shown `lygia::filtre::…` behind the scenes. LYGIA is licensed separately from this project, under the
Prosperity Public License 3.0.0 or the Patron License: see
[`vendor/lygia/VENDORED.md`](vendor/lygia/VENDORED.md).

One dialect gotcha when porting snippets: `mod` is a reserved word in WESL (it
declares a module), so write the builtin out as `x - y * floor(x / y)` instead
of calling `mod(x, y)`.

The pipeline: the editor text is linked by the WESL compiler, the composed
WGSL is parsed and validated with `naga` (same major version as the wgpu
inside Bevy 0.20, so type errors surface in the UI instead of the browser
console), then handed to Bevy as `Shader::from_wgsl`. The previous shader keeps
drawing until the new one's pipelines are built, so a compile never blanks
the screen, and a broken edit never replaces a working shader.

## Live diagnostics (wgsl-analyzer)

While you edit, the [wgsl-analyzer](https://github.com/wgsl-analyzer/wgsl-analyzer)
language server checks the code and the editor squiggles what it reports;
the status bar shows the count. Its workspace mirrors the real compile:
`shaders/shadertoy.wesl` is the hidden `package::shadertoy` module, `lygia/`
the bundled LYGIA (a path dependency in `wesl.toml`), and the editor text is
`shaders/user.wesl` (`package::user`), so imports and types resolve exactly as
when compiling.

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
  lib.rs        the Bevy-free modules below (compile, tweak, examples) as a
                library, for tools such as crates/backend-diff
  compile.rs    WESL link + naga validate (the shader contract above), with
                the bundled LYGIA as the `lygia` package
  tweak.rs      tweakable constants: find literal consts, rewrite them into
                uniform-fed `var<private>`s, write tweaked values back
  persist.rs    document/settings IO: localStorage, shader.wesl, file picker
  theme.rs      Gruvbox dark egui Visuals + the syntax palette
  headless.rs   `--render`: windowless wgpu renderer writing a PNG (native)
  render/
    mod.rs                    ShaderToyUniform/Fragment components + plugin
    active.rs                 which shader is on screen: switches once the
                              new pipelines are built, restarts i_frame,
                              channel clocks and buffers on the switch
    buffers.rs                the buffer_a..buffer_d passes (ping-pong
                              Rgba32Float targets behind i_channel_0..3)
    previews.rs               the example browser's live thumbnails: each
                              example's buffer and image passes into a small
                              image of its own
    blur.rs                   frosted-glass backdrop: the finished frame,
                              downsampled 4x and Gaussian-blurred into an
                              image egui paints under windows and panels
    fullscreen_material.rs    vendored from Bevy 0.20, modified to take its
                              fragment shader per view at runtime
  debugger.rs   debugger overlay (ported from bevy_shadertoy_wgsl): blit pass,
                overlay WGSL, font atlas rasterized from egui's Hack font
  highlight.rs  WESL/WGSL tokenizer -> colored LayoutJob
  editor.rs     the code editor: TextEdit with a custom layouter that paints
                the highlighted job, line-number gutter, Tab/Shift-Tab
  examples.rs   the built-in examples (examples/*.wesl, embedded by build.rs,
                which also parses each file's name/author/license header)
  browser.rs    the Examples window: cards, filters, lazy compiles, and which
                thumbnails to draw each frame
  analyzer/     wgsl-analyzer LSP client (live diagnostics)
    mod.rs        handshake, full-text sync, publishDiagnostics -> char ranges
    native.rs     the wgsl-analyzer executable over stdio
    web.rs        wgsl-analyzer-web in a Web Worker, via web/analyzer.js
examples/       one .wesl file per example; the leading comment block names
                and credits it (see src/examples.rs for the format)
vendor/lygia/   the WESL modules of LYGIA at a pinned commit, refreshed by
                vendor/update-lygia.sh; build.rs embeds them as a package
crates/
  backend-diff/ test crate (not part of the app or its wasm build):
                renders shaders on every wgpu adapter and diffs the pixels
    gpu.rs        offscreen buffer + image passes on one adapter
    image.rs      per-pixel diff, tolerances, PNG output
    translate.rs  naga's GLSL ES 3.00 / MSL / HLSL / SPIR-V writers, checked
                  by glslangValidator / spirv-val when installed
    fuzz.rs       seeded generator of well-typed main_image shaders
web/
  analyzer.js               JS side of the web transport
  fetch-wgsl-analyzer.sh    fetches the pinned wgsl-analyzer-web build
  coi-serviceworker.js      COOP/COEP for hosts without header control (MIT)
```

Rendering is the vendored Bevy `fullscreen_material` plugin: the user shader
runs as a fullscreen pass in `Core3d`'s post-process stage with
`Tonemapping::None`, so colors hit the screen untouched, after the buffer
passes. Per-frame uniforms ride in a dynamic uniform buffer bound at
`@group(0) @binding(2)`, the buffer channels at bindings 3..6.

Compiles alternate between two shader assets, replacing the one that is not
on screen. Bevy then rebuilds that asset's pipelines in place, so the set of
shaders and pipelines stays fixed however often you compile (Bevy's pipeline
cache never frees a pipeline). While the
debugger overlay is on, the user shader renders into an offscreen
`Rgba16Float` target instead (`Rgba8UnormSrgb` where WebGL2 cannot render
to half floats), and a second pass draws it plus the overlay to the view;
while it is off, that pass and the target cost nothing.

The UI is egui (via `bevy_egui`, `EguiPrimaryContextPass`), following the
integration pattern proven in [omdurman](https://github.com/barafael/omdurman).

## Development

```sh
cargo run              # native (dynamic Bevy, fast rebuilds)
cargo test             # compile pipeline, examples, highlighter, editor
cargo run -- --render Plasma out.png [--time 3] [--debugger]  # headless PNG
cargo run -- --example Fluid --examples  # start on an example, browser open
WGSL_ANALYZER=/path/to/wgsl-analyzer cargo test -- --ignored  # + live server
cargo test -p backend-diff                # every example on Vulkan vs GL + naga writers
cargo run -p backend-diff -- --fuzz 200   # the same as a table, plus fuzz shaders
trunk serve            # browser build on http://127.0.0.1:8080
```

Pinned toolchain: see `rust-toolchain.toml` (stable + `wasm32-unknown-unknown`).
Release wasm is built with `trunk build --release` (trunk ≥ 0.21 for the
`data-wasm-opt-params` flag — Rust emits bulk-memory ops that trunk's bundled
wasm-opt needs told about).

The repository is a Cargo workspace: the app is the root package, and
`crates/backend-diff` is a test harness for naga and wesl. It composes each
shader exactly as the app does, renders it (buffer passes included) on every
wgpu adapter the machine offers — Vulkan, and GL through EGL, which runs
naga's GLSL ES output like WebGL2 does — and diffs each render against the
Vulkan one in the sRGB bytes the screen shows: a pixel differs when a channel
is off by more than 8/255, and a case differs when over 1% of its pixels do.
Differing cases leave both renders, an amplified diff and the shader in
`target/backend-diff/`. The rendering tests skip with a note when there are
not two adapters; the naga translation tests need no GPU. Elevated, Fire,
Seascape and Voronoi are expected to differ: their `fract(sin(dot(p, K)) *
43758.5453)` hash turns a last-bit difference in `dot` (which drivers may
fuse into an FMA in either operand order) into a different random value.

CI and the GitHub Pages deployment live in `.github/workflows/ci.yml`
(fmt, clippy and tests on the native build of the whole workspace; the Pages
build uses trunk v0.22.0 with `--public-url /<repo>/`).

## License

MIT OR Apache-2.0. `src/render/fullscreen_material.rs` is vendored (and
modified) from Bevy 0.20 under the same dual license — see its header. The
debugger overlay in `src/debugger.rs` is ported from
[eliotbo/bevy_shadertoy_wgsl](https://github.com/eliotbo/bevy_shadertoy_wgsl)
(MIT OR Apache-2.0).

Two parts carry their own licenses: each file in `examples/` names its author
and license in its header (the shadertoy ports are mostly CC BY-NC-SA 3.0 or
CC BY-NC 2.0), and `vendor/lygia/` is LYGIA under the Prosperity Public
License 3.0.0 or the Patron License (see `vendor/lygia/VENDORED.md`).
