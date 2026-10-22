# WESL in Bevy: Practical Developer Experience Today (October 2026)

## How do you compile WESL today (crate API, CLI, build.rs/cargo integration)?

### Takeaway
The canonical Rust compiler is the `wesl` crate (v0.6.0, released 2026-10-04; note the crate is published as `wesl`, not `wesl-compiler`, and the current entry point is `Compiler::compile()` returning a `CompileResult` — there is no `compile_to_wgsl`). You can compile at runtime, at build time via `build.rs` + `include_wesl!`, or offline with the `wesl-cli` binary (`wesl compile <file> > out.wgsl`).

### Cited Findings
- The main crate is `wesl` on crates.io ("This is the crate for all your WESL needs"), v0.6.0 published 04 October 2026, dual MIT/Apache-2.0, maintained by `k2d222`; repository is `webgpu-tools/wesl-rs` (formerly `wgsl-tooling-wg/wesl-rs`) — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/), [crates.io/crates/wesl](https://crates.io/crates/wesl), [GitHub](https://github.com/webgpu-tools/wesl-rs).
- Runtime API (0.6.0): `let compiler = Compiler::default(); let result = compiler.compile("path/to/shader.wesl")?; let wgsl = result.syntax.to_string();` — errors pretty-print via `Display` (the crate depends on `annotate-snippets` for rustc-style diagnostics). There are also standalone `compile()` and `compile_async()` functions — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).
- Build-time API (0.6.0): add `wesl` to `[build-dependencies]`, in `build.rs` call `compiler.compile("src/shaders/main.wesl")`, then `compile_result.emit_rerun_if_changed()` and `compile_result.write_artifact("my_main_shader")` (writes `$OUT_DIR/my_main_shader.wesl`), and in app code use `source: ShaderSource::Wgsl(include_wesl!("my_main_shader"))` — [docs.rs/wesl 0.6.0 "Usage in build.rs"](https://docs.rs/wesl/latest/wesl/).
- The official Getting-Started-Rust page shows a slightly different, older-looking API (`wesl::Wesl::new("src/shaders").build_artifact(&"package::main".parse().unwrap(), "my_shader")` and `Wesl::new(...).compile(...)`), and the docs.rs README snippet still pins `wesl = "0.4"` — evidence of documentation lagging the 0.6.0 API (see pain points) — [wesl-lang.dev Getting Started Rust](https://wesl-lang.dev/docs/Getting-Started-Rust).
- CLI: `cargo install wesl-cli`; `wesl compile <path/to/shader.wesl> > compiled.wgsl`; reads stdin if no input given. Subcommands: `check` (validate), `compile` (to WGSL), `eval` (const-expression evaluation), `exec` (run a WGSL function on the CPU), `package` ("Generate a publishable Cargo package from WESL source code") — [wesl-cli 0.6.0 on docs.rs](https://docs.rs/crate/wesl-cli/latest), [wesl-lang.dev Getting Started](https://wesl-lang.dev/docs/Getting-Started-Rust).
- `wesl-cli` 0.6.0 released 2026-10-04; release cadence is roughly monthly: 0.4.0 (2026-05-19), 0.4.1 (2026-07-24), 0.4.2 (2026-08-05), 0.4.3 (2026-08-14), 0.4.4 (2026-08-23), 0.5.0 (2026-09-13), 0.6.0 (2026-10-04) — [docs.rs/crate/wesl-cli/latest](https://docs.rs/crate/wesl-cli/latest).
- Feature flags on `wesl` 0.6.0: `generics` (generic functions/type generators, experimental), `package` (shader libraries published to crates.io, experimental), `eval` (CPU execution + `@const` attributes, experimental), `naga-ext` (Naga/WGPU extensions, experimental), `serde` — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).
- Project status per README (last updated 2026-05): WESL 0.2 supports imports + inline import paths, `@if`/`@elif`/`@else` conditional compilation, and Cargo shader packages; experimental: evaluation/execution, const-expression lowering, validation; planned: automatic bindings/structs of bindings, namespaces/inline modules, generic functions — [wesl-rs README](https://raw.githubusercontent.com/webgpu-tools/wesl-rs/main/README.md).
- C bindings (`wesl-c`) and JS/TS tooling (`wesl-js`) also exist — [docs.rs/wesl-c](https://docs.rs/wesl-c), [wesl-js](https://github.com/wgsl-tooling-wg/wesl-js).

### Inferences
- For a Bevy app, the build.rs/`include_wesl!` path is mainly relevant for embedded shaders that don't need hot-reloading; Bevy's own asset pipeline (see next section) already does runtime compilation, so most Bevy users never call the `wesl` crate directly.
- The `package` subcommand + `package` feature means shader libraries are distributed as ordinary crates.io crates ("WESL packages can be used in your shader projects like any other Rust crates" — [bevy-wgsl repo](https://github.com/wgsl-tooling-wg/bevy-wgsl)).

### Gaps
- I could not verify the historical `wesl-compiler` crate name or a `compile_to_wgsl` function in any current source; current docs only show `wesl` / `Compiler::compile`. Treat `wesl-compiler`/`compile_to_wgsl` as outdated names.
- No dedicated cargo subcommand or build-plugin (e.g., a `cargo-wesl`) was found; integration is via `build.rs` or the CLI.

## How do you wire WESL into Bevy (official support, AssetLoader, real examples)?

### Takeaway
As of Bevy 0.20 (released October 8, 2026) you do not need any custom wiring: WESL is Bevy's official shader language, `.wesl` files load directly through `AssetServer`/`ShaderRef`, and the old naga_oil-based "Custom Bevy Extended WGSL" dialect has been removed. Before 0.20 it was an opt-in feature (`shader_format_wesl`, Bevy 0.16+); a custom AssetLoader approach is now unnecessary and unofficial.

### Cited Findings
- **Bevy 0.20 (2026-10-08), PR [#25088](https://github.com/bevyengine/bevy/pull/25088)** (authors @tychedelia, @BenjaminBrienen): "Bevy's shaders are now written in WESL and the old 'Custom Bevy Extended WGSL' language support has been removed… we believe it is better for the wider shader ecosystem (and for us) to adopt a common standard… We've been working closely with the WESL team." Plain WGSL files with no preprocessor directives keep working; GLSL support was removed; SPIR-V passthrough unchanged — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20), [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20).
- Adoption timeline: PR [#17953](https://github.com/bevyengine/bevy/pull/17953) first landed experimental `.wesl` shader loading (March 2025) — [This Week in Bevy 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization); Bevy 0.16 shipped it gated behind the `shader_format_wesl` cargo feature, "disabled by default", noting "WESL is still relatively early in development, and not all of its features are fully functional yet, nor are all its features supported in Bevy (yet)" — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16); Bevy 0.19 said "We've supported WESL for more than a year, but we're planning to port our existing internal shaders to use WESL, and endorse it as the shader language of choice for Bevy" — [Bevy 0.19 release notes](https://bevy.org/news/bevy-0-19); a WGSL tooling working group formed May 2025 — [This Week in Bevy 2025-05-05](https://thisweekinbevy.com/issue/2025-05-05-new-working-groups-wesl-first-party-tilemaps).
- The `bevy_shader` crate (split out of `bevy_render` in Bevy 0.17 — [0.16→0.17 migration guide](https://bevy.org/learn/migration-guides/0-16-to-0-17)) is the integration point: v0.20.0 (2026-10-08) depends on `wesl ^0.6.0` as a **non-optional** dependency (it was optional in [0.18](https://crates.io/crates/bevy_shader/0.18.0/dependencies) and [0.19](https://crates.io/crates/bevy_shader/0.19.1/dependencies)) and provides `ShaderLoader` (the `AssetLoader` for shader assets), `ShaderResolver` ("A Wesl import resolver. Maps module paths to actual Wesl shader source"), `ShaderCache` ("cache for shaders and shader imports, with asset state-tracking for waiting to load shaders until all imports are resolved"), `ShaderDefVal`, `ShaderRef`, and `ValidateShader` — [docs.rs/bevy_shader 0.20.0](https://docs.rs/bevy_shader/latest/bevy_shader/).
- Practical usage in 0.20 is just: `const SHADER_ASSET_PATH: &str = "shaders/custom_material.wesl";` with `ShaderRef` from `bevy::shader`, and inside the shader `import bevy_pbr::render::forward_io::VertexOutput;` plus `@group(2) @binding(constants::MATERIAL_BIND_GROUP)` — the official `shader_material` example now uses a `.wesl` file with relative imports (`import super::custom_material_import::COLOR_MULTIPLIER;`) — [Bevy shader-material example](https://bevy.org/examples/shaders/shader-material).
- Migration syntax rules (0.19→0.20): `#import` → `import …;` (semicolon, must come first in file); `#ifdef X` → `@if(X)` (also `@elif`/`@else`, attachable to declarations, struct members, function parameters, imports, statements); boolean shader defs become conditional-compilation flags; `Int`/`UInt` defs are readable as `constants::NAME` and enable a same-named flag; `#{...}` interpolation and `#define_import_path` are gone; `embedded://` shaders are importable at their crate+file path (`embedded://bevy_foo/bar.wesl` → `bevy_foo::bar`), other shaders at their asset path; the `shader_format_wesl` feature is gone (always enabled) — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20).
- Community crates already target this: `bevy_enoki` 0.8.0 (2026-10-11) documents custom materials written as `.wesl` importing `bevy_enoki::shaders::particle_vertex_out::VertexOutput` — [lib.rs/bevy_enoki](https://lib.rs/crates/bevy_enoki). The 0.20 release notes also show `MaterialExtension2d::fragment_shader() -> Some("my_material.wesl".into())` for new sprite materials — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20).
- `bevy-wgsl` (WG repo `wgsl-tooling-wg/bevy-wgsl`) is "all Bevy shaders converted to WESL" as an importable WESL package — this predates in-tree 0.20 adoption and is likely superseded by Bevy's own embedded WESL shaders (unofficial/experimental) — [GitHub](https://github.com/wgsl-tooling-wg/bevy-wgsl).

### Inferences
- For a Bevy 0.20 app, "adding WESL" is a non-event: write `.wesl`, load it via `AssetServer`/`ShaderRef`/materials, and Bevy's `ShaderLoader` + `ShaderResolver` compile it with the `wesl` crate at load time. Writing your own `AssetLoader` for `.wesl` would only make sense on Bevy ≤0.19 (unofficial pattern; I found no published gist implementing it) or for build-time precompilation via `build.rs`.
- Because `ShaderCache` tracks import dependencies ("waiting to load shaders until all imports are resolved"), imported-module dependency handling that used to be manual/fragile in the naga_oil era is now engine-managed.

### Gaps
- I found no blog post or gist demonstrating a hand-rolled `.wesl` → WGSL `AssetLoader` (the pre-0.20 unofficial route); with 0.20 shipping first-party support two weeks ago, that pattern is now largely moot.
- Exact Bevy 0.20 hot-reload semantics for `.wesl` files and their transitive imports are not documented in the sources I found (see pain points).

## What tooling exists for editing (VS Code extension, LSP)?

### Takeaway
The primary tool is **wgsl-analyzer** — an LSP language server (rust-analyzer-style, WGSL + WESL aware) with an official VS Code extension, plus a broader WGSL/WESL tool family (WGSL Studio, wgsl-test, playground) from the same WGSL tooling working group. Bevy's 0.20 release explicitly points users to wgsl-analyzer for IDE support.

### Cited Findings
- **wgsl-analyzer VS Code extension**: publisher `wgsl-analyzer` (marketplace item `wgsl-analyzer.wgsl-analyzer`), described as "WGSL/WESL support for Visual Studio Code", ~25,567 installs, 13 ratings; bundles the language server binary; features include code completion, go-to-definition, inlay hints for types, syntax highlighting (including inside host-language strings and Markdown code blocks); also on open-vsx — [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=wgsl-analyzer.wgsl-analyzer), [open-vsx](https://open-vsx.org/extension/wgsl-analyzer/wgsl-analyzer).
- wgsl-analyzer is "Language server for WGSL/WESL, based on rust-analyzer", multi-editor (VS Code, Emacs, Neovim, any LSP editor), incremental and error-resilient (analysis continues through syntax errors), cross-module go-to-definition/hover/diagnostics across the module graph, semantic highlighting, autocomplete across modules and libraries, multi-error reporting, and a standard formatter — [wesl-lang.dev wgsl-analyzer page](https://wesl-lang.dev/docs/wgsl-analyzer.html), [GitHub](https://github.com/wgsl-analyzer/wgsl-analyzer). Bevy 0.20 notes: "language server protocol support, in the form of wgsl-analyzer… syntax highlighting, go-to-definition, inlay hints, code folding, formatting and more, once installed for your IDE of choice" (with a screenshot of a Bevy fog shader highlighted) — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20).
- **WGSL Studio VS Code extension**: publisher `webgpu-tools` (item `webgpu-tools.wgsl-studio`), "WGSL/WESL previews and tests… includes a native test runner for GPU shader tests" — [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=webgpu-tools.wgsl-studio). Related tools listed on the WESL site: `wgsl-test` (unit tests for WGSL compute shaders), `wgsl-play`, `wgsl-edit` — [wesl-lang.dev docs nav](https://wesl-lang.dev/docs/wgsl-analyzer.html).
- Browser playground with live WESL→WGSL translation (and Bevy-flavored shader-toy rendering): [play.wesl-lang.dev](https://play.wesl-lang.dev); Bevy 0.20 notes call it out ("render pretty shader toys! live in your browser in the WESL Playground") — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20).
- The wgsl-analyzer page on wesl-lang.dev is headed "…coming soon" while the marketplace extension is live with 25k+ installs — the docs page appears stale relative to the shipping extension (confirmed shipping by the Bevy 0.20 notes and the marketplace listing itself).

### Inferences
- The WGSL/WESL editor story in Oct 2026 is: wgsl-analyzer for authoring/diagnostics, WGSL Studio for preview/testing, playground for quick experiments — all from the same `webgpu-tools`/WGSL-tooling-WG umbrella, so WESL syntax (`import`, `@if`) is first-class in tooling rather than an afterthought.
- There is no WESL-specific VS Code extension separate from wgsl-analyzer; the old naga_oil-era WGSL analyzer ecosystem converged on this one LSP.

### Gaps
- I did not find a separate dedicated "WESL" marketplace extension, nor any JetBrains/Gap-IDE shader support.
- No hard data on wgsl-analyzer's support for Bevy-specific `constants::` shader-def injection (the analyzer sees the `.wesl` file, but Bevy-injected shader defs come from the pipeline at load time) — unverified either way.

## What is the debugging story (source maps, error mapping, inspecting generated WGSL)?

### Takeaway
Debugging is the weakest part of the workflow: WESL compile-time errors are good (rustc-style diagnostics with source spans, plus resilient multi-error diagnostics in wgsl-analyzer), and the `wesl` crate exposes a `sourcemap` module, but I found no evidence of end-to-end mapping of *runtime WGSL/naga/wgpu validation errors* back to `.wesl` line numbers in Bevy; the practical escape hatch is dumping the generated WGSL with `wesl compile` or the playground.

### Cited Findings
- `wesl` 0.6.0 has a `sourcemap` module ("`SourceMap` trait and implementations") alongside the compiler API — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).
- Compile errors from the crate pretty-print with `Display` ("pretty errors with `display()`"), backed by the `annotate-snippets` crate (the rustc-style snippet renderer) — [wesl-lang.dev Getting Started](https://wesl-lang.dev/docs/Getting-Started-Rust), [docs.rs/wesl 0.6.0 dependencies](https://docs.rs/wesl/latest/wesl/).
- The `eval` feature "helps developers to catch bugs early by improving the validation and error reporting capabilities of WESL"; const-expression evaluation/`@const` precomputation and CPU execution (`wesl exec`) are tested against the WebGPU CTS (`k2d222/wesl-cts`) — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).
- In-editor: wgsl-analyzer reports multiple errors simultaneously and keeps type-checking valid code even when other parts of the file are broken — [wesl-lang.dev wgsl-analyzer](https://wesl-lang.dev/docs/wgsl-analyzer.html).
- Inspecting generated WGSL: `wesl compile shader.wesl > out.wgsl` (stdout by default), `wesl check` for validation only, and the browser playground shows the translation live — [wesl-cli docs](https://docs.rs/crate/wesl-cli/latest), [play.wesl-lang.dev](https://play.wesl-lang.dev).
- On the Bevy side, `bevy_shader` 0.20 exposes `ShaderCacheError` ("error returned by a `PipelineCache` when the creation of a GPU pipeline object failed") and `ValidateShader` (opt-in runtime checks on shaders; off by default) — [docs.rs/bevy_shader 0.20.0](https://docs.rs/bevy_shader/latest/bevy_shader/).
- The WESL project pitches "improving the validation and error reporting capabilities" as an eval-feature goal, and the 3D-on-Web 2025 talk describes codegen/linker extension APIs — [docs.rs/wesl](https://docs.rs/wesl/latest/wesl/), [talk](https://www.youtube.com/watch?v=Na1XTKK_Mig).

### Inferences
- Because Bevy compiles `.wesl` → WGSL with the `wesl` crate *before* handing WGSL to wgpu/naga, WESL-level errors (imports, `@if`, syntax) should surface against `.wesl` spans, but a WGSL validation error from naga/wgpu would reference the *composed* WGSL text — I found no source confirming span remapping of those errors back to `.wesl` files, so treat WGSL→WESL runtime error mapping as an open question.
- The pragmatic debugging loop people use today is likely: fix editor diagnostics (wgsl-analyzer) → run the app and read Bevy's pipeline error → `wesl compile` the entry file to inspect the flattened WGSL.

### Gaps
- No documentation or blog found on Bevy 0.20 mapping naga/wgpu shader-creation errors back to `.wesl` source positions, nor on any debug flag that dumps the composed WGSL from within a running Bevy app.
- No source-map support for GPU debuggers (e.g., RenderDoc seeing `.wesl` names) was found; presumably debuggers see only generated WGSL.

## What are known pain points or limitations reported by users?

### Takeaway
Reported/verifiable pain points: the LR(1) parser rejects some valid WGSL; very fast version churn (0.4→0.6 in five months, with docs lagging the API); several core features still experimental (generics, eval, packaging); Bevy 0.20's switch forces a shader-dialect migration; and the ecosystem is only now catching up to 0.20, which shipped two weeks ago.

### Cited Findings
- **Parser limitations**: wesl-rs "uses a LR(1) parser"; issue #162 (open as of research) shows valid WGSL (`if`-conditioned struct assignment) failing to compile, with the workaround "put the statement into a block" — [wesl-rs issue #162](https://github.com/wgsl-tooling-wg/wesl-rs/issues/162).
- **Version churn**: `wesl-cli` went 0.4.0 (2026-05-19) → 0.5.0 (2026-09-13) → 0.6.0 (2026-10-04) — near-monthly breaking-ish releases; `bevy_shader` pinned to `wesl ^0.6.0` in Bevy 0.20 — [docs.rs/wesl-cli](https://docs.rs/crate/wesl-cli/latest), [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/).
- **Docs lag the API**: the official Getting-Started-Rust page demonstrates `wesl::Wesl::new(...)` while docs.rs 0.6.0 shows `Compiler::compile`/`CompileResult`; the docs.rs front page's build.rs snippet still says `wesl = "0.4"`; the wesl-lang.dev wgsl-analyzer page still says "coming soon" though the extension ships — [Getting Started](https://wesl-lang.dev/docs/Getting-Started-Rust) vs [docs.rs/wesl](https://docs.rs/wesl/latest/wesl/); [wgsl-analyzer page](https://wesl-lang.dev/docs/wgsl-analyzer.html).
- **Experimental surface**: README status (2026-05): generics, namespaces/inline modules, automatic bindings are *planned/not designed yet*; eval, lowering, validation are *experimental* — [wesl-rs README](https://raw.githubusercontent.com/webgpu-tools/wesl-rs/main/README.md). Bevy 0.16 similarly warned "not all of its features are fully functional yet, nor are all its features supported in Bevy (yet)" — [Bevy 0.16 notes](https://bevy.org/news/bevy-0-16).
- **Migration cost in Bevy 0.20**: every custom shader in the naga_oil dialect must be translated (`#import`→`import`, `#ifdef`→`@if`, `#{...}`→`constants::`, module paths restructured to match crate paths, e.g. `bevy_pbr::prepass_utils` → `bevy_pbr::prepass::utils`) and renamed `.wgsl`→`.wesl`; GLSL support was removed outright — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20).
- **Hot-reloading history**: Bevy hot-reloads file-based shaders when the asset watcher is enabled ([Unofficial Bevy Cheat Book](https://bevy-cheatbook.github.io/assets/hot-reload.html)), but hot-reload of *imported* shaders regressed and broke in the naga_oil era (issue #10500, Bevy 0.12) — [bevy#10500](https://github.com/bevyengine/bevy/issues/10500). `bevy_shader` 0.20's `ShaderCache` does track import state, but I found no current report confirming or denying correct hot-reload of `.wesl` import graphs.
- **Ecosystem lag**: Bevy 0.20 released 2026-10-08, so third-party material/render crates are mid-migration (e.g., bevy_enoki shipped 0.8.0 on 2026-10-11) — [bevy.org/news/bevy-0-20](https://bevy.org/news/bevy-0-20), [lib.rs/bevy_enoki](https://lib.rs/crates/bevy_enoki).

### Inferences
- The pain points are typical of a ~18-month-old language toolchain: parser edge cases, moving APIs, and docs that trail releases — but the direction of travel is clearly consolidation (Bevy removing its own dialect in favor of WESL, one LSP covering WGSL+WESL).
- Teams adopting WESL with Bevy today should pin exact `bevy`/`wesl` versions and expect to re-verify on each Bevy release, since `bevy_shader` hard-pins a specific `wesl` minor version.

### Gaps
- I found no Reddit/Discord threads (quoted in blogs or GitHub) with first-hand user complaints about WESL compile errors, hot-reload behavior, or IDE gaps in the Bevy 0.16–0.20 `shader_format_wesl` window; the pain points above are from trackers, docs, and release notes rather than aggregated user sentiment.
- No benchmark of WESL compile times (runtime shader load cost) was found.

## Are there community projects using WESL with Bevy or wgpu?

### Takeaway
Yes, and the flagship user is now Bevy itself (0.20 runs on WESL); around it sit the WGSL-tooling-WG tool family (playground, wgsl-test, WGSL Studio, examples, CTS), shader libraries packaged as WESL crates (`bevy-wgsl`, Lygia's WebGPU port), and community Bevy plugins (bevy_enoki) shipping `.wesl` shaders.

### Cited Findings
- **Bevy / bevy_shader** (official): depends on `wesl ^0.6.0`, all engine shaders authored in WESL — [docs.rs/bevy_shader 0.20.0](https://docs.rs/bevy_shader/latest/bevy_shader/), [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20).
- **bevy-wgsl** (`wgsl-tooling-wg/bevy-wgsl`): "all Bevy shaders converted to WESL… WESL packages can be used in your shader projects like any other Rust crates" (pre-0.20 community package; likely superseded by in-tree adoption) — [GitHub](https://github.com/wgsl-tooling-wg/bevy-wgsl).
- **bevy_enoki** (2D particle plugin): custom-material docs use `.wesl` with `import bevy_enoki::shaders::particle_vertex_out::VertexOutput;`; 0.8.0 released 2026-10-11 — [lib.rs/bevy_enoki](https://lib.rs/crates/bevy_enoki).
- **Lygia** (well-known shader library): its WebGPU README instructs using the WESL CLI for custom build pipelines ("use the wesl-link command-line tool to statically link shaders. install wesl-cli; wesl compile <path/to/shader.wesl>") — [lygia README_WebGPU.md](https://github.com/patriciogonzalezvivo/lygia/blob/main/README_WebGPU.md).
- **WGSL tooling WG examples repo**: example WESL projects — WGSL compute shaders with unit tests, WebGPU PNG generation, command-line/custom build pipelines with no runtime library — [wgsl-tooling-wg/examples](https://github.com/wgsl-tooling-wg/examples).
- **wesl-cts**: fork of the WebGPU Conformance Test Suite used to test wesl-rs eval/exec — [docs.rs/wesl](https://docs.rs/wesl/latest/wesl/), [github.com/k2d222/wesl-cts](https://github.com/k2d222/wesl-cts).
- **wesl-js**: WESL runtime linking for JavaScript/TypeScript (npm-published shader imports) — [GitHub](https://github.com/wgsl-tooling-wg/wesl-js).
- **WESL Playground** renders shader-toy-style demos in-browser and is promoted by Bevy's release notes — [play.wesl-lang.dev](https://play.wesl-lang.dev), [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20).
- Community discussion of the compiler exists (e.g., a Reddit r/golang post about a Go WESL compiler implementation noting imports + `@if` passing the spec tests) — [r/golang thread](https://www.reddit.com/r/golang/comments/1tncffs/wgsl_extension_language_wesl_compiler).

### Inferences
- The `webgpu-tools` GitHub org (formerly `wgsl-tooling-wg`) is effectively the center of gravity: spec, wesl-rs, wesl-js, examples, playground, analyzer, and studio all live there, with Bevy as the largest downstream consumer — adoption outside Bevy (Lygia, standalone wgpu apps via CLI/build.rs) is real but thinner.

### Gaps
- I could not retrieve the full crates.io reverse-dependency list for `wesl` (API fetch failed with 400/403 through the fetch proxy), so the list above is from targeted searches, not an exhaustive enumeration; there are likely additional small wgpu/Bevy crates depending on `wesl` (e.g., via `bevy_shader`'s dependents page).
- No dedicated "shader-toy-in-Bevy-with-WESL" hobby project was found by name; the closest official artifact is the WESL Playground itself.
