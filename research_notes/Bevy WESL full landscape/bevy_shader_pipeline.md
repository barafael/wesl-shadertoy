# Bevy's Shader Pipeline Architecture and Evolution (0.12 → 0.20), and Where WESL Fits

Scope: shader source pipeline only (parsing, preprocessing/imports, specialization, caching), not general rendering. All version dates and claims below are from primary Bevy sources (release notes, migration guides, docs.rs) unless noted.

Release timeline used throughout: 0.13 (Feb 2024), 0.14 (Jul 2024), 0.15 (Nov 2024), 0.16 (Apr 24 2025), 0.17 (~Sep 2025), 0.18, 0.19, 0.20 (Oct 8 2026, latest as of Oct 2026) — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/), [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/), [0.16→0.17 migration guide](https://bevy.org/learn/migration-guides/0-16-to-0-17/).

## Q1. How does Bevy's current (0.20) shader pipeline work, and where does WESL sit in it?

### Takeaway
As of 0.20, Bevy shaders are written in WESL (a strict superset of WGSL) by default: the old bespoke "Custom Bevy Extended WGSL" dialect and its naga_oil-based preprocessor were removed outright, and WESL translation is now an unconditional, always-on stage inside the `bevy_shader` crate's `ShaderLoader`/`ShaderResolver`/`ShaderCache` flow, before the composed WGSL reaches wgpu/Naga.

### Cited Findings
- Bevy 0.20 (released Oct 8 2026) ships "WESL Shaders" (PR #25088, @tychedelia / @BenjaminBrienen): shaders are now written in WESL ([wesl-lang.dev](https://wesl-lang.dev)), and the old "Custom Bevy Extended WGSL" dialect was removed — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/)
- "The naga_oil preprocessor is gone"; the `shader_format_wesl` cargo feature is gone — WESL support is always enabled — [Bevy 0.20 migration guide (0.19→0.20)](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- Migration mechanics: shaders written in the old dialect must be translated to WESL and renamed `.wgsl` → `.wesl`; plain WGSL with no preprocessor directives keeps working unchanged — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- Syntax translation examples: `#import bevy_pbr::forward_io::VertexOutput` → `import bevy_pbr::render::forward_io::VertexOutput;`; `#ifdef X` → `@if(X)`; bind-group placeholder `#{MATERIAL_BIND_GROUP}` → `constants::MATERIAL_BIND_GROUP` — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- Module resolution rule: an import path like `bevy_foo::bar` resolves to the `bevy_foo` crate if such a crate exists, otherwise to an asset path — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- 0.20 added wgsl-analyzer LSP support for Bevy shaders — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/); wgsl-analyzer is listed as a WESL-ecosystem tool — [wesl-lang.dev](https://wesl-lang.dev/)
- The `bevy_shader` crate (v0.20.0) has a hard dependency on `wesl ^0.6.0` — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- Key `bevy_shader` 0.20 API surface (docs.rs, verbatim descriptions):
  - `Shader` — "An 'unprocessed' shader. It can contain imports and conditional compilation attributes."
  - `ShaderLoader` — "The AssetLoader responsible for loading unprocessed shader assets."
  - `ShaderResolver` — "A Wesl import resolver. Maps module paths to actual Wesl shader source."
  - `ShaderCache` — "A cache for shaders and shader imports, with asset state-tracking for waiting to load shaders until all imports are resolved."
  - `ShaderCacheSource` — "Fully composed source code of a shader module, with all shader defs applied."
  - `ShaderDefVal` — "A compile time shader value definition to be inlined into the shader source."
  - `ShaderImport` — "A shader import, described as either an asset path or an import path."
  - `CachedPipelineId` — "Typically corresponds to a unique combination of `Shader` and `ShaderDefVal`s."
  - `ValidateShader` — runtime shader checks off by default "for performance" (ties to 0.16's "unchecked shaders" change).
  - `load_shader_library!` macro — "Inline shader as an embedded_asset and load it permanently."
  — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- WESL itself: "a strict superset of WGSL" adding imports, conditional compilation, and packages; existing `.wgsl` files work as-is; Rust/JS linkers are "< 20kb" and "easy to embed"; C bindings exist — [wesl-lang.dev](https://wesl-lang.dev/)
- When experimental WESL support first landed (PR #17953, merged 2025-03-09, @tychedelia), WESL was described as "a community-driven portable superset of WGSL", pre-MVP 0.1.x, intending to add "imports, conditional compilation, packaging, and more to WGSL, which aligns with how Bevy uses WGSL generally"; the PR enabled loading `.wesl` shader sources with a `shader_material_wesl` example, and noted you could "not expect to mix shaders via naga-oil yet" (i.e., WESL and the old naga_oil dialect did not interoperate) — [This Week in Bevy, 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization)

### Inferences
- The pipeline shape in 0.20 is: `.wesl` asset → `ShaderLoader` (AssetLoader) → `ShaderResolver` resolves WESL imports (crate-mapped or asset-path) → `ShaderCache` composes final WGSL source with `ShaderDefVal`s inlined → composed WGSL handed to wgpu/Naga; pipeline variants are cached per (Shader, ShaderDefVal) combination. (Inferred from the docs.rs API descriptions above.)
- Because WESL is a strict WGSL superset and plain WGSL still loads, the WESL translation step is effectively a no-op passthrough for vanilla `.wgsl`, which is why the 0.20 migration could keep `.wgsl` working — consistent with [wesl-lang.dev](https://wesl-lang.dev/) and the [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/).
- Bevy's decade-long ad-hoc preprocessor features (imports, `#ifdef`, shader defs, bind-group constants) were *replaced one-for-one* by standardized WESL syntax (`import`, `@if`, `ShaderDefVal` inlining, `constants::`), suggesting the project decided to outsource dialect maintenance to the community spec rather than keep a private extension.

### Gaps
- Exact internal call order inside `ShaderLoader::load` (e.g., where `wesl::Wesl` linker is invoked vs. where `ShaderDefVal`s are applied) is not documented on the docs.rs landing page; would need the source of `crates/bevy_shader/src/loader.rs`.
- Whether the composed WGSL is validated/compiled by Naga at shader-asset load time or deferred to pipeline creation in 0.20 is not sourced.

## Q2. How did the shader pipeline evolve 0.12 → 0.19 (preprocessor, crate split, specialization)?

### Takeaway
The arc is: naga_oil-based custom WGSL extension gradually pushed out of core crates (0.13) → shader infrastructure extracted into a dedicated `bevy_shader` crate and specialization made composable (0.17) → naga_oil removed entirely in favor of WESL (0.20), with experimental WESL as the bridge (0.16).

### Cited Findings
- **0.13 (Feb 2024):** merged PR "Remove naga_oil dependency from bevy_pbr" (changelog entry #11914) — naga_oil was still used elsewhere but PBR shaders stopped depending on it — [Bevy 0.13 release notes](https://bevy.org/news/bevy-0-13/)
- **0.14 (Jul 2024):** no shader-preprocessor changes found in the release notes (searched for CrossShader/cross-shader/MetaShader/naga_oil/HLSL/type injection; only naga-related limitation note, see Q4) — [Bevy 0.14 release notes](https://bevy.org/news/bevy-0-14/)
- **0.14→0.15:** wgpu 0.20 / naga 0.20 / naga_oil 0.14 dependency bumps; "All of Naga's Capabilities should now be properly detected and supported" — [0.14→0.15 migration guide](https://bevy.org/learn/migration-guides/0-14-to-0-15/)
- **0.15 (Nov 2024):** `ShaderStorageBuffer` became a first-class asset type — [Bevy 0.15 release notes](https://bevy.org/news/bevy-0-15/)
- **0.16 (Apr 24 2025):** "Experimental WESL Shaders" (PR #17953): `.wesl` loading gated behind opt-in `shader_format_wesl` cargo feature, off by default; release notes explicitly describe Bevy's then-current approach as its *own* WGSL extension providing conditional compiling and imports — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/)
- **0.16:** GPU-driven rendering groundwork: bindless resources via `#[bindless]` attribute on `AsBindGroup` types, and "cached pipeline specialization" — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/); bindless storage buffer rework PRs (#17994, #18125, @pcwalton) landed the same month — [This Week in Bevy, 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization)
- **0.16:** "Use unchecked shaders for better performance" (skip runtime validation) — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/); the 0.20 `ValidateShader` type (default: no runtime checks) is the surviving API for this — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- **0.17 (~Sep 2025):** `Shader`, `ShaderRef`, `ShaderDef`, `ShaderCache`, `PipelineCompilationError` moved out of `bevy_render` into the new `bevy_shader` crate, enabling "shader library only crates with minimal dependencies" — [0.16→0.17 migration guide](https://bevy.org/learn/migration-guides/0-16-to-0-17/)
- **0.17:** "Composable Specialization" (PR #17373): the old `SpecializedRenderPipeline` trait pattern was replaced by a `Specializer` trait plus `Variants<T, S>` keyed cache, where a base pipeline descriptor is mutated per variant key and specializers compose via derive — [0.16→0.17 migration guide](https://bevy.org/learn/migration-guides/0-16-to-0-17/), [Bevy 0.17 release notes](https://bevy.org/news/bevy-0-17/)
- **0.19→0.20:** naga_oil fully removed; `shader_format_wesl` feature removed (WESL unconditional); old extended-WGSL dialect deleted — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- **0.20:** mesh shaders integrated with the pipeline cache (`MeshPipeline`/`MeshPipelineDescriptor`, PR #25627) — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/)
- By 0.20, `bevy_shader` depends on `bevy_asset`, `bevy_reflect`, `wesl ^0.6.0`, `wgpu-types ^30` — i.e., the shader crate is asset-pipeline-coupled but renderer-agnostic — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)

### Inferences
- The 0.13 "remove naga_oil from bevy_pbr" step and the 0.16 "experimental WESL behind a feature" step show a two-phase deprecation: first shrink naga_oil's blast radius, then offer a standards-based alternative behind a flag, only flipping the default in 0.20 once WESL matured (0.1.x in Mar 2025 → 0.6 dependency by Oct 2026).
- The 0.17 `bevy_shader` split was likely a prerequisite for the WESL switch: putting `Shader`/`ShaderCache`/`ShaderDef` in a minimal crate with `wesl` as its only preprocessing dependency made the dialect swap a single-crate change.
- `ShaderDefVal` surviving the WESL switch (still in the 0.20 API, "inlined into the shader source") means Bevy's keyword-based shader defines were kept as a specialization mechanism *on top of* WESL's own `@if` conditional translation, rather than being replaced by it.

### Gaps
- **CrossShader (reportedly a 0.13/0.14-era experiment to share shader code across backends/dialects):** found no reliable sources. GitHub code/issue search (unauthenticated API) returned 0 results for "CrossShader"; the 0.13 and 0.14 release notes and the 0.12→0.15 migration guides contain no mention; web searches returned nothing. Cannot confirm it existed, what it did, or when it was reverted.
- **MetaShader (reportedly a 0.14-era proposed unified shader-description abstraction):** found no reliable sources; same negative searches as CrossShader.
- Which Bevy version introduced `ShaderDefVal`/`AsBindGroupShaderType` originally (pre-0.17, likely 0.11–0.12) is unsourced here — the migration guides I fetched don't cover it.
- What exactly happened to naga_oil usage between 0.13 (removed from bevy_pbr) and 0.19 (fully gone) — e.g., whether other crates kept it via a fallback path — is not documented in the pages fetched.

## Q3. What lessons came from HLSL support attempts?

### Takeaway
I found no reliable primary-source evidence of an HLSL support effort (`bevy_hlsl` or otherwise) in Bevy 0.12–0.20; searches across release notes, migration guides, GitHub search, and the web all came up empty, so any "HLSL lessons" claim should be treated as unverified.

### Cited Findings
- Bevy 0.13/0.14 release notes contain no mention of HLSL (full-text searched) — [0.13](https://bevy.org/news/bevy-0-13/), [0.14](https://bevy.org/news/bevy-0-14/)
- 0.12→0.20 migration guides contain no HLSL mentions (searched 0.14→0.15, 0.16→0.17, 0.19→0.20) — [migration guides index pattern](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- The 0.20 shader story is explicitly WGSL-centric (WESL = WGSL superset; output is WGSL for wgpu) with no cross-compilation-from-other-languages discussion — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/)

### Inferences
- If an HLSL experiment existed (e.g., a community `bevy_hlsl` crate or a Naga HLSL-frontend path), it left no trace in official Bevy release notes/migration guides, implying it never reached core or was quietly abandoned — consistent with the general WESL narrative of "standardize on WGSL-family, don't fragment."
- The closest *sourced* "support limitation" lesson is Naga's WebGL2 depth-sampling failure (see Q4), which shows Bevy's multi-backend story is bounded by Naga/wgpu's WGSL-subset handling, not by input-language frontends.

### Gaps
- **`bevy_hlsl` crate/experiment:** found no reliable sources. Unauthenticated GitHub search returned 0 results for "bevy_hlsl"; web search for Bevy HLSL experiment/PR returned nothing. Cannot state what it was or what lessons were drawn.
- Whether Naga's HLSL *frontend* (historically used by wgpu for parsing HLSL inputs) was ever exercised in Bevy is unsourced.

## Q4. What is Naga's role, and which WGSL limitations motivated extensions/WESL?

### Takeaway
Naga is the WGSL compiler/translator underneath wgpu that Bevy feeds final WGSL to, and Bevy's release history documents concrete cases where Naga's capabilities/bugs dictated shader features (WebGL2 depth sampling) — while the language-level gaps that Bevy's preprocessor (and later WESL) fill are imports, conditional compilation, and code sharing, which WGSL itself lacks.

### Cited Findings
- Bevy 0.14: screen-space reflections and depth-of-field "unsupported on WebGL2" because "naga can't [sample depth textures there]… sampler2DShadow incorrectly generated" — a concrete Naga limitation that gated a Bevy rendering feature — [Bevy 0.14 release notes](https://bevy.org/news/bevy-0-14/)
- 0.14→0.15: Bevy bumped to naga 0.20 / naga_oil 0.14 / wgpu 0.20, and stated "All of Naga's Capabilities should now be properly detected and supported" — showing Bevy's feature-gating reads Naga's capability model — [0.14→0.15 migration guide](https://bevy.org/learn/migration-guides/0-14-to-0-15/)
- WGSL (as WESL positions itself) lacks imports, conditional compilation, and packaging — the exact features WESL adds, and the exact features Bevy's custom dialect had hand-rolled — [wesl-lang.dev](https://wesl-lang.dev/)
- Bevy's own release notes describe its pre-0.20 dialect as a custom WGSL extension adding conditional compiling and imports — i.e., Bevy hit the same limitations the WESL spec targets — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/)
- WESL landed in Bevy as `.wesl` loading via the `wesl` Rust crate (0.1.2 at the time of the Mar 2025 PR; `wesl ^0.6.0` by Bevy 0.20) — [This Week in Bevy, 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization), [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- "Decoupled naga for wgpu testing" (PR #18099, @JMS55) landed Mar 2025, letting Bevy test newer wgpu/Naga versions independently of its pinned release — evidence Naga version coupling was a real friction point — [This Week in Bevy, 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization)

### Inferences
- Bevy's motivation for WESL is dual: (a) language gaps WGSL has (imports/conditionals — the preprocessor problem) and (b) tooling/ecosystem leverage (wgsl-analyzer LSP, community libraries like lygia) that a private dialect could never get — both supported by [wesl-lang.dev](https://wesl-lang.dev/) and the [0.20 notes](https://bevy.org/news/bevy-0-20/).
- Naga remains the "last stop" in every version studied: Bevy never shipped non-WGSL-family shader sources to GPUs; every dialect (extended WGSL, WESL) compiles *down to* WGSL before Naga/wgpu.

### Gaps
- Deeper catalog of Naga/WGSL limitations (e.g., lack of bitcast ergonomics, no templates/generics, pointer/address-space pain) is discussed in the community doc "Some Thoughts on Bevy WGSL" ([GitHub discussion #14823](https://github.com/bevyengine/bevy/discussions/14823)) and the rendering summary hackmd ([hackmd.io/@bevy/rendering_summary](https://hackmd.io/@bevy/rendering_summary)), but I could not retrieve either (hackmd returned 403 to the fetcher; discussion not fetched within budget) — treat specifics from those as unverified.

## Q5. What custom WGSL extensions did Bevy have (type injection, shader defs, bindless), and what happened to each in 0.20?

### Takeaway
Bevy's pre-0.20 stack was: naga_oil-backed imports/`#ifdef`/`#{}` placeholders, `ShaderDefVal` keyword injection, and (from 0.16) bindless `AsBindGroup` codegen; in 0.20 the first group was replaced by WESL syntax (`import`, `@if`, `constants::`), while `ShaderDefVal` and bindless specialization survived as Rust-side APIs feeding the WESL pipeline.

### Cited Findings
- Pre-0.20 dialect features (imports, conditional compilation) were provided by Bevy's custom WGSL extension with naga_oil as preprocessor — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/), [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- 0.20 removed that dialect; replacements: `#import` → `import …;`, `#ifdef` → `@if(…)`, `#{MATERIAL_BIND_GROUP}` → `constants::MATERIAL_BIND_GROUP` — [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- `ShaderDefVal` ("compile time shader value definition to be inlined into the shader source") still exists in 0.20's `bevy_shader`, and `ShaderCacheSource` is "fully composed source code … with all shader defs applied" — type/keyword injection survives — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- Bindless: 0.16 introduced `#[bindless]` on `AsBindGroup` types + cached pipeline specialization — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/); bindless storage buffers were reworked the same month (PRs #17994, #18125) — [This Week in Bevy, 2025-03-17](https://thisweekinbevy.com/issue/2025-03-17-wesl-recursive-runsystem-and-transform-propagation-optimization)
- Pipeline identity is still (Shader, ShaderDefVal…) keyed via `CachedPipelineId` in 0.20 — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)

### Inferences
- The `#{}` → `constants::` change means bind-group layout placeholders moved from *textual* substitution to ordinary WGSL constant references — removing a class of "magic syntax invisible to WGSL tooling" (wgsl-analyzer can parse the new form), which matches WESL's stated "standard extensions, tool-supported" philosophy — [wesl-lang.dev](https://wesl-lang.dev/).
- Bevy's specialization story (shader defs + `Specializer`/`Variants`) is deliberately *orthogonal* to the source dialect: swapping naga_oil→WESL required no change to how variants are keyed/cached.

### Gaps
- Whether `@if` in WESL-Bevy reads `ShaderDefVal`s directly (i.e., how defs are surfaced to the WESL linker's conditional-translation context) is not documented on the pages fetched; would need PR #25088 details or `bevy_shader` source.

## Q6. Where does a WESL preprocessor integrate in Bevy's asset pipeline (relevant for a Shadertoy-style WESL host)?

### Takeaway
In 0.20 the integration point is fully inside `bevy_shader`: `ShaderLoader` (a standard Bevy `AssetLoader`) reads `.wesl` sources, a `Wesl`-backed `ShaderResolver` maps import paths to crate or asset sources, and `ShaderCache` produces composed WGSL with shader defs applied — all before anything touches wgpu — so a custom host can reuse `bevy_shader` (or the `wesl` crate directly) as a self-contained preprocessing layer.

### Cited Findings
- `ShaderLoader` is "The AssetLoader responsible for loading unprocessed shader assets"; `ShaderResolver` is "A Wesl import resolver. Maps module paths to actual Wesl shader source"; `ShaderCache` tracks asset state "waiting to load shaders until all imports are resolved" — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- Imports can be "either an asset path or an import path" (`ShaderImport`), and crate-vs-asset resolution is: crate name if the crate exists, else asset path — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/), [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- `load_shader_library!` embeds a shader library via `embedded_asset` and loads it permanently — the mechanism Bevy crates use to expose their `.wesl` libraries — [docs.rs/bevy_shader](https://docs.rs/bevy_shader/latest/bevy_shader/)
- The 0.17 crate split explicitly made "shader library only crates with minimal dependencies" possible — i.e., consuming shaders without the renderer is a supported use case — [0.16→0.17 migration guide](https://bevy.org/learn/migration-guides/0-16-to-0-17/)
- Historically the format switch was a cargo-feature/`ShaderFormat`-style dispatch (`shader_format_wesl` in 0.16–0.19; removed in 0.20 when WESL became the only dialect) — [Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16/), [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- The `wesl` crate itself is designed to be embedded: linkers "< 20kb", "easy to embed in editor applications", C bindings available, plus a translation playground — [wesl-lang.dev](https://wesl-lang.dev/)

### Inferences
- For a Shadertoy-like host on Bevy 0.20: register `.wesl` (and optionally `.wgsl`) assets, let `ShaderLoader`+`ShaderResolver` compose, then feed `ShaderCacheSource` into a single render pipeline — no need for `AsBindGroup`/material machinery; the 0.17+ `bevy_shader` crate exists precisely to allow that minimal footprint.
- Hot-reloading shader source in 0.20 should work through normal `Assets<Shader>` reload semantics since `Shader` is a plain asset and `ShaderCache` tracks import resolution state — consistent with the docs.rs descriptions, though reload behavior itself is not explicitly documented.
- A pre-0.20 host (0.16–0.19) would have had to choose between two mutually incompatible preprocessors (naga_oil dialect vs. feature-gated WESL) — the TWIW note that you couldn't "mix shaders via naga-oil yet" implies mixed-dialect projects were painful before 0.20 unified them.

### Gaps
- The exact `ShaderFormat`/file-extension dispatch code in 0.16–0.19 (how `.wesl` vs `.wgsl` loaders were registered) is unsourced — docs.rs for older `bevy_shader` versions wasn't fetched.
- Whether third parties can inject a *custom* preprocessor step into `ShaderLoader` in 0.20 (e.g., via a pluggable resolver trait) is not documented on the landing page; `ShaderResolver` appears to be Wesl-specific.

## Source Reliability Notes
- Primary sources used: bevy.org release notes and migration guides (official), docs.rs/bevy_shader 0.20.0 (generated from the crate itself), wesl-lang.dev (WESL spec site), thisweekinbevy.com (community-run but PR-accurate, links PR numbers/authors/dates).
- GitHub's unauthenticated search API returned 0 results for terms that plausibly exist ("CrossShader", "metashader", "bevy_hlsl"), so absence of GitHub hits was *not* treated as evidence of absence; only the absence from official release notes/migration guides is reported as fact.
- hackmd.io/@bevy/rendering_summary returned HTTP 403 (JS-gated) and could not be mined; GitHub discussion #14823 ("Some Thoughts on Bevy WGSL") was identified but not fetched — both remain leads for the report writer rather than sources.
