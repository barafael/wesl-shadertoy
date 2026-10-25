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
| Examples | `Examples` window — 41 examples (a seven-part tutorial, Plasma, Seascape, Fluid, a dozen LYGIA demos, …) as live thumbnails, the tutorial and other beginner examples first; search or filter by tag, hover to play with one (the pointer drives its `iMouse`), click to open it. Fresh sessions roll a random one |
| Tweak constants | `Constants` window: a slider or color picker per literal module-level constant, live while you drag (see [Tweakable constants](#tweakable-constants)) |
| Evolve | toolbar `Evolve`: the canvas becomes a 3×3 grid, the current constants in the middle and eight mutations around them, all running live (hover one to drive its `iMouse`). Click a variant to breed from it (the centre rolls eight new ones), **Ctrl/Cmd+Z** or the undo button walks back the lineage, the slider sets how far mutations stray. **Keep** writes the centre's constants into the code; **Discard** or **Escape** leaves it unchanged |
| Browse LYGIA | `LYGIA` window: search the ~380 bundled modules by path, function name or description, filter by directory; each entry shows the module's doc text and its WGSL declarations, an **import** button per declaration adds the `import lygia::…;` line to your code, **source** opens the module read-only |
| Feature flags | `Flags` window: a checkbox per WESL `@if` flag your code and its LYGIA imports test, a value per `constants::` setting they read; changes recompile (see [Feature flags and constants](#feature-flags-and-constants)) |
| Windows | `Examples`, `Code`, `WGSL` (composed output), `Constants`, `Flags`, `LYGIA` toggles; `⚙` settings (frosted-glass blur slider, 0–24 px: how blurred the shader shows through the UI, 0 = off), `?` about |
| Edit | Tab / Shift-Tab indent, line-number gutter, syntax highlighting |
| Live diagnostics | squiggles + gutter marks from [wgsl-analyzer](https://github.com/wgsl-analyzer/wgsl-analyzer) while you type; hover for the message; toggle in `⚙` |
| `iMouse` | click + drag on the canvas (clicks on UI are ignored) |
| The shader eats the editor | toolbar droplet icon (enabled while the shader reads `i_ui`, e.g. the `UI Melt` example): the rendered UI goes into the shader as a texture and the shader's output is what you see, so it can ripple, smear or melt the editor you type into. Clicks and typing still land on the undistorted UI. **F2** always switches it off; it is never saved, and it pauses the frosted glass and the debugger overlay (see [The UI as a texture](#the-ui-as-a-texture)) |
| Cost heatmap | toolbar flame icon (or `--heat` natively): the image becomes a thermal map of loop iterations per pixel, black (none) through red to pale yellow (the legend's max, 256 by default, dragged live in the canvas corner) — see [Cost heatmap](#cost-heatmap) |
| Debugger overlay | toolbar bug icon (or `--debugger` natively): crosshair and click ring, RGBA probe boxes for the pixels under the cursor and the last click, a swatch of the clicked pixel's color, mouse position, resolution, fps |

The document auto-saves to `localStorage` (browser builds) on every change —
edits, opened examples and files — and is restored on load, along with your
settings. Compile errors appear in the bottom panel — the last good shader keeps
running while you fix them, also when the GPU driver rejects a shader that
passed validation.

## Tutorial

New to shaders, or to this app? The Examples window starts with seven short
examples (tag `tutorial`), each about one feature and commented to be read
top to bottom; each card says what it teaches:

1. **Hello Shader** — `main_image`, `frag_coord`, `i_resolution`, `i_time`
2. **Tweak Hints** — live constants and their `// tweak:` hints
3. **Mouse Input** — what `i_mouse()` reports
4. **Imports** — WESL imports and your own functions
5. **Buffers 101** — a `buffer_a` feedback trail and `i_frame`
6. **LYGIA 101** — importing LYGIA functions, and their conventions
7. **Debugger Tour** — reading values back with the debugger overlay

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

**Evolve** mode mutates the same constants, after
[Picbreeder](https://en.wikipedia.org/wiki/Picbreeder): each variant moves
some of them by a random step scaled to their range. Annotated ranges are never
left, colors stay within 0..1, unannotated numbers keep their sign, and integer
constants mutate rarely, since they tend to change a shader's structure.

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
| `i_ui_sample(uv: vec2<f32>) -> vec4<f32>` | — | the rendered UI at `uv` (premultiplied; transparent unless the UI texture mode is on, see [below](#the-ui-as-a-texture)) |

`frag_coord` is in pixels with the origin **bottom-left**, like shadertoy's
`fragCoord` (and `i_mouse()`); the wrapper flips WGSL's top-left
`@builtin(position)` for you.

### Cost heatmap

The flame icon recompiles the shader with every `loop`, `for` and `while`
body counting itself, and shows the count per pixel instead of the image.
Raymarchers light up where they work hardest — silhouettes, grazing angles,
fog — which is where a bounding volume, a lower step count or an earlier
exit pays off. Helper functions count too, and nested loops multiply. Only
the editor's own loops are counted (not LYGIA's), only the image pass is
shown (buffer passes keep running as usual), and constants stay tweakable.

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

### The UI as a texture

With the toolbar's droplet on, the image pass also receives the rendered UI
(toolbar, windows, panels, everything) and its output replaces the UI on
screen:

```rust
import package::shadertoy::{i_resolution, i_time, i_ui_sample};

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var uv = frag_coord / i_resolution().xy;
    uv.x += 0.004 * sin(uv.y * 60.0 + i_time() * 4.0);  // wobble the UI
    let ui = i_ui_sample(uv);                            // premultiplied
    let bg = vec3<f32>(0.1, 0.07, 0.15);
    return vec4<f32>(ui.rgb + bg * (1.0 - ui.a), 1.0);
}
```

`i_ui_sample(uv)` samples it bilinearly at `uv` in 0..1 with the origin
bottom-left (like `frag_coord / i_resolution().xy`); the raw `i_ui` texture
(row 0 at the top) and its `i_ui_sampler` are importable too. Colors are
premultiplied by alpha, which is 0 where no UI is drawn. While the mode is
off, and in buffer passes, thumbnails and `--render`, `i_ui` is a transparent
1×1 texture, so the shader shows just its background. The toggle only enables
for shaders that read `i_ui` (any other would hide the UI) and switches off
when a compile stops reading it.

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
browse the modules in the **LYGIA** window, in [`vendor/lygia`](vendor/lygia)
or on [lygia.xyz](https://lygia.xyz). The window is built at compile time
from each module's doc block (description, contributors, `use:` lines,
license, notes on its flags) plus the declarations the file actually makes:
the `use:` lines are often GLSL-flavoured and out of date, so the WGSL
signatures are what it lists. Its **import** button adds
`import lygia::path::to::module::name;` after your last import (or extends a
one-line import of the same module, `import lygia::m::{a, b};`), and does
nothing if the name is imported already. It edits the document in place,
like the Constants window's write-back, so autosave picks it up.

LYGIA's 2D shapes are centered on (0.5, 0.5),
and a few (like `heartSDF`) assume y points down, unlike `frag_coord`. Some
tonemappers (`tonemapFilmic3`, `tonemapUnreal3`) bake in gamma 2.2, which the
sRGB output then applies a second time. Its `lighting::raymarch` helpers are
still stubs in the WESL port, so bring your own marcher. The twelve
`Lygia …` examples show it in use, from noise, palettes, tilings and easing
to marbling (`distort::marble` across two buffers), post-processing filters
over a buffer, and a raymarched Cook-Torrance material study; Perlin,
Voronoi and Fire take their noise from it too. The editor's
wgsl-analyzer still rejects `filter` (a WGSL reserved word) as a module name,
which WESL allows, so it is shown `lygia::filtre::…` behind the scenes. LYGIA is licensed separately from this project, under the
Prosperity Public License 3.0.0 or the Patron License: see
[`vendor/lygia/VENDORED.md`](vendor/lygia/VENDORED.md).

### Feature flags and constants

WESL compiles code under `@if(FLAG)` only while `FLAG` is on, and reads
settings from its virtual `constants::` module. Many LYGIA modules use them:
`CENTER_2D` moves the center of the 2D SDFs, `YUV_SDTV` switches
`rgb2yuv` from BT.709 to BT.601, `MIXOKLAB_SRGB` mixes in sRGB, and
`distort::marble` reads its period from `constants::MARBLE_PERIOD`. The
**Flags** window lists every flag and constant your code and the LYGIA
modules it imports (directly or through other modules) test, with the
modules and the doc notes behind each. Flags change which code exists, so
unlike tweaks they recompile. They are kept in your code as comment lines,
so saved and shared shaders compile the same way:

```rust
// wesl-flags: CENTER_2D, YUV_SDTV
// wesl-const: CENTER_2D = vec2f(0.5, 0.25)
// wesl-const: MARBLE_PERIOD = 90.0
```

A constant's value is any WGSL expression (`const NAME = value;` in the
`constants` module), and setting it also turns on the flag of the same
name: LYGIA guards `import constants::MARBLE_PERIOD;` with
`@if(MARBLE_PERIOD)`. Your own code can test flags (`@if(BRIGHT)`) and
import constants too. wgsl-analyzer knows nothing about these lines, and
it treats every `@if` branch as present: flags inside LYGIA do not bother
it (it only checks your code), but in your own code a declaration made once
per branch (`@if(X) const K = 1.0; @else const K = 2.0;`) shows as a
"duplicate identifier", and `import constants::…` as a missing package. The
compile itself is right in both cases.

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
  compile.rs    WESL link + naga validate (the shader contract above), with
                the bundled LYGIA as the `lygia` package
  tweak.rs      tweakable constants: find literal consts, rewrite them into
                uniform-fed `var<private>`s, write tweaked values back
  heatmap.rs    cost heatmap: count loop iterations in a `var<private>`,
                the heat entry point and palette, the legend

  flags.rs      the Flags window: `// wesl-flags:` / `// wesl-const:` lines
                -> WESL features and the `constants::` module, and which
                flags a shader's (transitive) LYGIA imports test
  library.rs    the LYGIA window: the module index build.rs generates,
                search, and the import button's edit
  wesl_scan.rs  lexical scans of WESL text (import trees, `@if` flags,
                `constants::` names), shared with build.rs
  persist.rs    document/settings IO: localStorage, shader.wesl, file picker
  theme.rs      Gruvbox dark egui Visuals + the syntax palette
  ui_texture.rs the UI texture mode's switch: toolbar toggle (only for
                shaders that read `i_ui`) and the F2 emergency off
  headless.rs   `--render`: windowless wgpu renderer writing a PNG (native)
  render/
    mod.rs                    ShaderToyUniform/Fragment components + plugin
    active.rs                 which shader is on screen: switches once the
                              new pipelines are built, restarts i_frame,
                              channel clocks and buffers on the switch
    buffers.rs                the buffer_a..buffer_d passes (ping-pong
                              Rgba32Float targets behind i_channel_0..3)
    previews.rs               live thumbnails (the example browser's and
                              Evolve's variants): each job's buffer and
                              image passes into an image of its own
    blur.rs                   frosted-glass backdrop: the finished frame,
                              downsampled 4x and Gaussian-blurred into an
                              image egui paints under windows and panels
    ui_pass.rs                the UI texture mode: clears the view for egui,
                              then runs the image pass after egui with the
                              UI bound as `i_ui`
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
  evolve.rs     Evolve mode: mutating constants, the lineage, the 3×3 grid of
                variants drawn as live thumbnails
  analyzer/     wgsl-analyzer LSP client (live diagnostics)
    mod.rs        handshake, full-text sync, publishDiagnostics -> char ranges
    native.rs     the wgsl-analyzer executable over stdio
    web.rs        wgsl-analyzer-web in a Web Worker, via web/analyzer.js
examples/       one .wesl file per example; the leading comment block names
                and credits it (see src/examples.rs for the format)
vendor/lygia/   the WESL modules of LYGIA at a pinned commit, refreshed by
                vendor/update-lygia.sh; build.rs embeds them as a package
                and indexes their doc blocks and declarations
web/
  analyzer.js               JS side of the web transport
  fetch-wgsl-analyzer.sh    fetches the pinned wgsl-analyzer-web build
  coi-serviceworker.js      COOP/COEP for hosts without header control (MIT)
```

Rendering is the vendored Bevy `fullscreen_material` plugin: the user shader
runs as a fullscreen pass in `Core3d`'s post-process stage with
`Tonemapping::None`, so colors hit the screen untouched, after the buffer
passes. Per-frame uniforms ride in a dynamic uniform buffer bound at
`@group(0) @binding(2)`, the buffer channels at bindings 3..6, `i_ui` and
its bilinear sampler at 7 and 8. In the UI texture mode the image pass moves
behind egui instead: the view is cleared to transparent, egui draws into it,
and the user shader reads that as `i_ui` and writes the frame; egui's input
and layout are untouched, which is why clicks still hit the undistorted UI.

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
cargo run -- --render Plasma out.png [--time 3] [--debugger] [--heat [--heat-max 256]]  # headless PNG
cargo run -- --example Fluid --examples  # start on an example, browser open
cargo run -- --example "UI Melt" --ui-texture  # start with the shader eating the UI
WGSL_ANALYZER=/path/to/wgsl-analyzer cargo test -- --ignored  # + live server
trunk serve            # browser build on http://127.0.0.1:8080
```

Pinned toolchain: see `rust-toolchain.toml` (stable + `wasm32-unknown-unknown`).
Release wasm is built with `trunk build --release` (trunk ≥ 0.21 for the
`data-wasm-opt-params` flag — Rust emits bulk-memory ops that trunk's bundled
wasm-opt needs told about).

CI and the GitHub Pages deployment live in `.github/workflows/ci.yml`
(fmt, clippy and tests on the native build; the Pages build uses trunk
v0.22.0 with `--public-url /<repo>/`).

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
