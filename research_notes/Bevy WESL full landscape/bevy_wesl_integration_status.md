# Bevy × WESL: Integration Status as of October 2026

## Is WESL officially part of Bevy? Is the wesl repo under the bevyengine org? Who maintains and funds it?

### Takeaway
As of Bevy 0.20 (released 2026-10-08), WESL is Bevy's official, mandatory shader language — but the WESL language and compiler themselves are **not** under the bevyengine GitHub org; they live in the independent cross-project `webgpu-tools` org (formerly `wgsl-tooling-wg`), led by Mathis Brossier (@k2d222). Bevy's support is deep engineering collaboration, not ownership; no public funding arrangement was found.

### Cited Findings
- **Not under bevyengine org**: A search of the bevyengine GitHub org for "wesl" repositories returns **0 repositories** ([org repo search](https://github.com/orgs/bevyengine/repositories?q=wesl)); `github.com/bevyengine/wesl` is a 404.
- **Actual repo location**: The compiler lives at [github.com/webgpu-tools/wesl-rs](https://github.com/webgpu-tools/wesl-rs) (146 stars, 905 commits, created 2024-08-19, last pushed 2026-10-19). The org `webgpu-tools` (created 2024-05-08, blog = wesl-lang.dev, 17 repos) was previously named `wgsl-tooling-wg` — old URLs like `wgsl-tooling-wg/wesl-rs` and `wgsl-tooling-wg/bevy-wgsl` redirect to `webgpu-tools/*` (observed via fetch redirects). The spec is at `webgpu-tools/wesl-spec`.
- **README's stated Bevy relationship**: "wesl-rs implements the necessary tools to build complex WGSL shaders, like what [naga_oil](https://github.com/bevyengine/naga_oil) does for [Bevy](https://bevyengine.org/), but in a framework-agnostic way" — [wesl-rs README](https://github.com/webgpu-tools/wesl-rs) (README "Status" section last updated 2026-05: WESL 0.2 released with imports, `@if/@elif/@else` conditional compilation, Cargo shader packages; experimental: eval/execution, const-expression lowering, validation; planned: automatic bindings, namespaces, generics).
- **Maintainership**: Top contributors to wesl-rs are **k2d222 = Mathis Brossier** (687 commits; GitHub bio: "PhD student in computer science at Linköping University, Sweden", blog thissma.fr), then stefnotch (65), **BenjaminBrienen (56)**, claydugo, jannik4, and others ([contributors API](https://api.github.com/repos/webgpu-tools/wesl-rs/contributors)). Note: Benjamin Brienen — whose GitHub bio says "Projects include `bevy` and `wgsl-analyzer`" ([profile](https://github.com/BenjaminBrienen)) — is the third-largest wesl-rs contributor, i.e., a Bevy-side developer contributing directly upstream.
- **Bevy's own characterization**: Bevy 0.19 release notes call WESL a "**cross-project group**" effort: "Bevy has been working together with a cross-project group to extend it, in the form of WESL" ([Bevy 0.19](https://bevy.org/news/bevy-0-19), June 19, 2026). Bevy 0.20: "we've been working with the WESL team to plan out the future of shader development in Bevy" ([Bevy 0.20](https://bevy.org/news/bevy-0-20)).
- **Funding**: No evidence found of Bevy (or any foundation) funding WESL. Searches for Bevy fund/grant support of WESL and NLnet grants for WESL turned up nothing specific. The `webgpu-tools` org lists no public members via the GitHub API. Bevy's support appears to be in-kind (contributor time, e.g., Brienen's 56 commits; wgsl-analyzer work; joint language design).

### Inferences
- WESL is "official" for Bevy in the sense of being the adopted, required shader language of the engine (0.20+), but it is an external open standard that Bevy helps shape — analogous to Bevy's relationship with WGSL/wgpu, not a bevyengine-owned project.
- The `bevy-wgsl` package (see below) is the one repo that is Bevy-specific yet maintained by the WESL team (k2d222), not by bevyengine.

### Gaps
- No public statement found on who *funds* WESL work (Mathis Brossier's Linköping PhD affiliation may imply academic support, but that is unverified speculation).
- Bevy Zulip archives were not directly accessible; working-group internal discussions could not be quoted.
- "Francesco 'MrDoor' Cretu" could not be tied to Bevy or WESL at all: GitHub user [MrDoor](https://github.com/MrDoor) exists but has no bio or verifiable Bevy/WESL connection. This name appears to be a false premise in the assignment.

## Do any Bevy releases ship WESL support? Is it wired into the asset pipeline?

### Takeaway
Yes, in three stages: **experimental behind a flag** (0.16–0.19, `shader_format_wesl`, off by default), then **fully adopted and unconditional** in **Bevy 0.20 (released 2026-10-08)** — WESL parsing/linking is wired into the `bevy_shader` crate's asset pipeline with no feature gate, and Bevy's own bundled shaders are all written in WESL.

### Cited Findings
- **Bevy 0.16** (released April 2025): "Experimental WESL Shaders" section, authored by @tychedelia, PR [#17953](https://github.com/bevyengine/bevy/pull/17953) ("Add support for experimental WESL shader source", opened 2025-02-20, **merged 2025-03-09**): adds `ShaderSource::WESL` for loading `.wesl` files; "WESL support is gated behind the cargo feature `shader_format_wesl`, which is disabled by default"; notes WESL shaders couldn't mix with naga_oil and had to pass through the naga frontend ([Bevy 0.16 release notes](https://bevy.org/news/bevy-0-16)). The feature is listed in `docs/cargo_features.md` at tags v0.16.0 and v0.19.0 ("shader_format_wesl — Enable support for shaders in WESL").
- **Bevy 0.17** (2025-09-30) and **0.18**: release notes contain **no WESL mentions** (verified by fetching both pages), though the gated feature remained. 0.17 did split out the separate `bevy_shader` crate ([Bevy 0.17 notes](https://bevy.org/news/bevy-0-17)).
- **Bevy 0.19** (2026-06-19): "We've supported WESL for more than a year, but we're planning to port our existing internal shaders to use WESL, and endorse it as the shader language of choice for Bevy" ([Bevy 0.19 notes](https://bevy.org/news/bevy-0-19)).
- **Bevy 0.20** (released **2026-10-08**, tagged by mockersf; [releases page](https://github.com/bevyengine/bevy/releases)): "WESL Shaders: Bevy has officially adopted the WESL shader language (a standardized extension of WGSL)... Bevy's shaders are now written in WESL and the old 'Custom Bevy Extended WGSL' language support has been removed." Section authors: @tychedelia, @BenjaminBrienen; PR [#25088 "Migrate to WESL"](https://github.com/bevyengine/bevy/pull/25088) (opened 2026-07-21, **merged 2026-08-05**; body: "Migrates Bevy's shaders to WESL via wesl-rs", linking `webgpu-tools/wesl-spec` and `webgpu-tools/wesl-rs`) ([Bevy 0.20 notes](https://bevy.org/news/bevy-0-20)).
- **Asset pipeline wiring (current main)**: `crates/bevy_shader/Cargo.toml` on `main` has an **unconditional dependency** `wesl = { version = "0.6.0", features = ["naga-ext"] }` ([raw Cargo.toml](https://raw.githubusercontent.com/bevyengine/bevy/main/crates/bevy_shader/Cargo.toml)). The `bevy::shader` module docs describe `ShaderResolver` as "**A Wesl import resolver.** Maps module paths to actual Wesl shader source", plus `ShaderCache` ("cache for shaders and shader imports, with asset state-tracking for waiting to load shaders until all imports are resolved") and `ShaderImport` ([docs.rs bevy::shader](https://docs.rs/bevy/latest/bevy/shader/index.html)).
- **Feature flag removed**: 0.19→0.20 migration guide: "The `shader_format_wesl` Cargo feature is gone, **WESL support is always enabled**. GLSL support has also been removed. SPIR-V passthrough is unchanged." Also: "the naga_oil preprocessor is gone. Custom shaders in the naga_oil dialect need to be translated to WESL and renamed from `.wgsl` to `.wesl`. Plain WGSL files with no preprocessor directives keep working"; `#define_import_path` is gone; `embedded://` shaders importable at crate paths ([migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20)). `docs/cargo_features.md` on main no longer mentions wesl at all (verified).
- **Ongoing post-adoption work** (Sept–Oct 2026, merged PRs found via [GitHub PR search](https://github.com/bevyengine/bevy/pulls?q=wesl)): #25440 "Use `import package` in wesl files" (2026-08-17), #25464 "Stop rendering escaped ANSI sequences in wesl errors" (2026-08-19), #25581 "wgsl-analyzer support" (2026-08-28), #25744 "Make downsample.wesl valid WESL" (2026-09-10), #25768 "Update WESL 0.6" (2026-09-13), #25783 "Fix spurious 404s when loading WESL shader imports on the web" (2026-09-14), #25921 "Update wesl requirement from 0.4.2 to 0.5.0" (2026-09-25), #26002 sprite_material.wesl warning fix (2026-10-01), #26070 "Update shader snippets in docs to WESL syntax" (2026-10-09); open issue [#25363 "Unresolved WESL import warning on web"](https://github.com/bevyengine/bevy/issues/25363).
- **wesl crate cadence**: `wesl` on crates.io: 0.1.0 (2025-02-20) → 0.2.0 (2025-07-28) → 0.3.x (Dec 2025) → 0.4.x (Jul–Aug 2026) → 0.5.0 (2026-09-13) → **0.6.0 (2026-10-04)**, description "The WESL compiler" ([crates.io API](https://crates.io/api/v1/crates/wesl)).

### Inferences
- The migration is fully shipped, not experimental: by 0.20 WESL is a hard dependency of every Bevy app, and the old naga_oil-based preprocessing path no longer exists in the engine.
- Bevy's `naga-ext` feature usage shows Bevy needs naga/wgpu extension types (64-bit types, extra builtins, ray-query API, etc.) that standard WGSL lacks — a concrete example of Bevy shaping WESL usage ([wesl Cargo.toml feature docs](https://raw.githubusercontent.com/webgpu-tools/wesl-rs/main/crates/wesl/Cargo.toml)).

### Gaps
- Whether the `bevy_shader` crate shipped with WESL *linking* already in 0.16–0.19 or only raw-source compilation could not be pinned down precisely from release notes; PR #17953's body indicates early versions compiled WESL then fed the result through naga, without naga_oil interop.

## What is the "bevy_compat" feature in the wesl-compiler crate?

### Takeaway
**No such feature could be found — this appears to be a false premise.** The compiler crate is named `wesl` (never `wesl-compiler` on crates.io), and none of its published feature sets (0.1.1, 0.3.0, 0.6.0) nor its sibling crate `wgsl-parse` contain a `bevy_compat` flag. Bevy compatibility was instead handled by Bevy-side integration and by the `bevy-wgsl` shader package.

### Cited Findings
- `crates.io/api/v1/crates/wesl-compiler` → **404**; the crate has always been published as `wesl` (versions from 0.1.0, 2025-02-20) ([crates.io](https://crates.io/crates/wesl)).
- Feature flags of `wesl` 0.6.0: `eval`, `generics`, `naga-ext`, `package`, `serde` — no `bevy_compat` ([docs.rs features page](https://docs.rs/crate/wesl/latest/features)). Same for 0.1.1 (`eval`, `generics`, `package`, `serde`; [docs.rs 0.1.1](https://docs.rs/crate/wesl/0.1.1/features)) and for the workspace `crates/wesl/Cargo.toml` at tag v0.3.0 and main ([raw Cargo.toml](https://raw.githubusercontent.com/webgpu-tools/wesl-rs/main/crates/wesl/Cargo.toml)).
- `wgsl-parse` features: `default`, `generics`, `naga-ext`, `serde`, `tokrepr`, `wesl` — no bevy flag ([docs.rs](https://docs.rs/crate/wgsl-parse/latest/features)).
- GitHub commit search for `bevy_compat` in `webgpu-tools/wesl-rs`: **no hits**; grep.app code search across GitHub finds `bevy_compat` only in the unrelated `aevyrie/big_space` crate (a local module name, not a cargo feature).
- What *does* exist that is Bevy-specific: (a) the **`bevy-wgsl`** repo — "Bevy internal shaders in a WESL package" ([github.com/webgpu-tools/bevy-wgsl](https://github.com/webgpu-tools/bevy-wgsl), created 2025-04-29, last pushed 2026-10-11, maintained by k2d222 with 39 of 41 commits), which exposes all Bevy shaders as a consumable WESL package with a long list of feature flags (BINDLESS, ALPHA_TO_COVERAGE, etc.) and links @jannik4's [shader docs](https://jannik4.github.io/shader_docs/bevy/latest/bevy/index.html); (b) Bevy's own former `shader_format_wesl` cargo feature (0.16–0.19, since removed); (c) the `naga-ext` feature Bevy enables.

### Inferences
- The "bevy_compat" claim likely conflates one of: Bevy's `shader_format_wesl` feature, the `bevy-wgsl` package, or early WESL's tolerance of Bevy's `#import`-style hash syntax (the 0.20 notes' "Before" example shows the old dialect used `#import`, `#ifdef`, and `#{MATERIAL_BIND_GROUP}` placeholders, [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20)).

### Gaps
- If `bevy_compat` existed transiently in an unpublished workspace crate (e.g., inside `wesl-frontend`, which has no docs.rs build), it left no trace in published crates, commit messages, or indexed code; I could not exhaustively search all historical file contents of wesl-rs.

## What have Bevy core devs said and done about WESL (PRs, discussions, statements)?

### Takeaway
The driving Bevy figure is **@tychedelia** (Bevy rendering/shader maintainer; GitHub name "charlotte", company @nannou-org), who authored both the experimental support PR (#17953) and the full migration PR (#25088), with **@BenjaminBrienen** (Bevy + wgsl-analyzer) co-authoring the 0.20 migration. **cart** and the org publicly endorsed WESL in 0.16 launch materials and 0.19/0.20 release notes. No significant public opposition was found.

### Cited Findings
- **tychedelia**: authored PR #17953 (merged 2025-03-09; objective: "WESL's pre-MVP 0.1.0 has been released!... so that we can begin playing and testing WESL, as well as aiding in their development") and PR #25088 "Migrate to WESL" (merged 2026-08-05) ([PR API data](https://api.github.com/repos/bevyengine/bevy/pulls/25088)). In the pre-WESL design discussion [#14823 "Some Thoughts on Bevy WGSL"](https://github.com/bevyengine/bevy/discussions/14823) (Aug 2024), tychedelia flagged tooling as the main shader pain point and proposed reviving `wgsl-analyzer` — the exact LSP later shipped with 0.20.
- **Benjamin Brienen (@BenjaminBrienen)**: co-author credited in the 0.20 "WESL Shaders" section ([Bevy 0.20 notes](https://bevy.org/news/bevy-0-20)); 3rd top wesl-rs contributor (56 commits); bio lists bevy and wgsl-analyzer ([profile](https://github.com/BenjaminBrienen)); Bevy 0.20 recommends wgsl-analyzer for "syntax highlighting, go-to-definition, inlay hints, code folding, formatting" ([0.20 notes](https://bevy.org/news/bevy-0-20)).
- **cart (Bevy lead)**: in the official Bevy 0.16 launch video, states WESL "is an early community-wide effort to extend WGSL with features like imports, conditional compilation, and even package management... it's super notable that this will also lead to better language server support in your editor. As of 0.16, Bevy implements partial support for WESL" ([YouTube: Bevy Engine 0.16](https://www.youtube.com/watch?v=l13mPxDvKLQ), WESL segment ~8:18).
- **Bevy org statements**: 0.16 notes — "The WESL team (who helped write these notes!) is actively listening to feedback, and so is Bevy" ([0.16](https://bevy.org/news/bevy-0-16)); 0.19 notes — endorse WESL as "the shader language of choice for Bevy" ([0.19](https://bevy.org/news/bevy-0-19)); 0.20 notes — "we believe it is better for the wider shader ecosystem (and for us) to adopt a common standard where we can pool resources on language improvements, module ecosystems, and IDE tooling. We've been working closely with the WESL team to evolve the standard in a way that fits well into the Bevy picture" ([0.20](https://bevy.org/news/bevy-0-20)).
- **Working group**: Bevy opened a dedicated **WESL working group** in May 2025: "The Bevy WESL-ification has begun! ... the working group to figure out what migrating would look like and do some testing has been created. Will WESL be enough for Bevy's needs?" ([This Week in Bevy, 2025-05-05](https://thisweekinbevy.com/issue/2025-05-05-new-working-groups-wesl-first-party-tilemaps); [video](https://www.youtube.com/watch?v=23d9MSi8V9U)). The working group is no longer listed on the current [bevy.org working-groups page](https://bevy.org/learn/contribute/project-information/working-groups) (no "wesl" match), consistent with the goal being completed by the 0.20 migration.
- **Alice Cecile**: no direct WESL-specific public statement was found; she is a Bevy co-lead who cut the v0.19.0 release ([releases](https://github.com/bevyengine/bevy/releases)) and appears in WESL-adjacent history only via general shader/tooling posts (e.g., a 2024 Mastodon post promoting a community Bevy shader language-server prototype, [Mastodon](https://mastodon.gamedev.place/@alice_i_cecile/112350055195478484)).

### Inferences
- The trajectory (2025-03 experimental merge → 2025-05 working group → 2026-06 endorsement → 2026-08 migration merge → 2026-10 0.20 ship) shows a deliberate ~18-month evaluation in which Bevy used WESL in-repo before committing; the "Will WESL be enough for Bevy's needs?" question was answered affirmatively by the 0.20 removal of naga_oil and the custom dialect.
- Bevy's shader leadership and the WESL team overlap personally (Brienen commits to both; the WESL team co-wrote Bevy's 0.16 release notes), indicating convergence rather than arm's-length adoption.

### Gaps
- Zulip threads and working-group meeting notes are not publicly fetchable here, so internal design debates (e.g., objections, alternatives considered) could not be cited.
- No public statement from cart or Alice Cecile *after* the 0.20 release about WESL was located.

## Relationship between Bevy's shader preprocessing/specializer and WESL: complementary, competing, or converging?

### Takeaway
**Converged.** WESL *replaced* Bevy's competing in-house preprocessor (naga_oil + "Custom Bevy Extended WGSL" dialect) in 0.20; Bevy's remaining specialization machinery (shader defs / pipeline specialization / BSN bindings) is complementary and now operates on top of WESL.

### Cited Findings
- **Replacement**: "Bevy's shaders are now written in WESL and the naga_oil preprocessor is gone" ([0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20)); the old dialect's `#import`, `#ifdef`, `#{BIND_GROUP}` placeholder syntax is gone, replaced by WESL `import`, `@if` attributes, and `constants::` names ("Int/UInt defs are readable as `constants::NAME` and enable a flag of the same name"); GLSL input also removed in the same cleanup.
- **Complementary specialization remains**: Bevy still has `ShaderDefVal` ("A compile time shader value definition to be inlined into the shader source") and pipeline specialization (`ShaderCacheSource`: "Fully composed source code of a shader module, with all shader defs applied") in `bevy::shader` ([docs.rs](https://docs.rs/bevy/latest/bevy/shader/index.html)); the 0.20 notes pair WESL with "BSN" (Bevy's shader-notation/binding metadata) work ([0.20 notes](https://bevy.org/news/bevy-0-20)).
- **Historical competition framing**: Bevy 0.16 notes acknowledged the overlap candidly: "Bevy has its own extended version of WGSL, which adds support for conditional compiling, importing between files... WESL is a brand new shader language that extends WGSL (**often in ways similar to Bevy's approach**)"; migration meant "Bevy's own 'extended WGSL syntax' will need to be ported to its WESL counterparts" ([0.16 notes](https://bevy.org/news/bevy-0-16)).
- **Upstream influence**: WESL's README positions itself as doing "what naga_oil does for Bevy... in a framework-agnostic way" ([wesl-rs README](https://github.com/webgpu-tools/wesl-rs)); WESL's `naga-ext` feature (used by Bevy) supports naga/wgpu-specific extensions ([wesl Cargo.toml](https://raw.githubusercontent.com/webgpu-tools/wesl-rs/main/crates/wesl/Cargo.toml)); WESL site markets against "app-specific custom string preprocessors" ([wesl-lang.dev](https://wesl-lang.dev/)).
- **Ecosystem alignment**: `bevy-wgsl` repackages Bevy shaders as a standard WESL/Cargo package so third parties can `import bevy_pbr::...` like any crate dependency ([bevy-wgsl README](https://github.com/webgpu-tools/bevy-wgsl)); Bevy merged "Use `import package` in wesl files" (#25440, 2026-08-17).

### Inferences
- Bevy effectively outsourced its language-extension layer (imports, cfg, packaging) to WESL while keeping engine-specific concerns (bind-group specialization, shader defs, BSN) in-house — a clean division of labor rather than duplication.
- naga_oil (still under the bevyengine org) is now legacy; nothing found indicates it's maintained for Bevy anymore.

### Gaps
- No post-0.20 roadmap document was found stating whether `ShaderDefVal` specialization might eventually be expressed via WESL conditional-compilation attributes (i.e., further convergence) — speculation territory only.

## Bevy examples, official example usage, and production adoption of WESL

### Takeaway
WESL had an official example (`shader_material_wesl`) from 0.16–0.19 when it was opt-in; after 0.20 made WESL the default, the dedicated example was removed and **the standard shader examples themselves are now WESL** (e.g., `custom_material.wesl`, `custom_clustered_decal.wesl`). No verified production game usage was found as of 2026-10-22.

### Cited Findings
- **0.16–0.19 (opt-in era)**: 0.16 notes point to "the new Material - WESL example"; the example file `examples/shader/shader_material_wesl.rs` exists at tag v0.19.0 ("A shader that uses the WESL shading language", loading `shaders/custom_material.wesl`) ([raw file at v0.19.0](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/shader/shader_material_wesl.rs)); the bevy.org example page noted it "requires this feature flag... `shader_format_wesl`" (page since 404s on current docs).
- **0.20 (default era)**: `shader_material_wesl` is gone from `main` (404), but the core `shader_material` example now uses WESL imports (`import bevy_pbr::render::forward_io::VertexOutput; import super::custom_material_import::COLOR_MULTIPLIER;` with `shaders/custom_material.wesl`) ([bevy.org example](https://bevy.org/examples/shaders/shader-material)); example assets like [`custom_clustered_decal.wesl`](https://github.com/bevyengine/bevy/blob/main/assets/shaders/custom_clustered_decal.wesl) live on main; PR #25439 fixed compile errors in `custom_stencil.wesl` / `specialized_mesh_pipeline.wesl` (2026-08-17).
- **Third-party tooling around Bevy WESL**: jannik4's shader docs generate documentation for Bevy's WESL shaders ([shader_docs](https://jannik4.github.io/shader_docs/bevy/latest/bevy/index.html), linked from bevy-wgsl README); the WESL Playground (play.wesl-lang.dev) is showcased in Bevy's own 0.20 notes.
- **Production adoption**: none verifiable. Bevy 0.20 shipped only 2026-10-08 (2 weeks before this research), and no game studio or shipped Bevy game was found publicly citing WESL. (A 2025-era "To Build a Home" and other showcases predate WESL adoption; the Medium article on 0.20 is paywalled commentary, not evidence of adoption.)

### Inferences
- Because plain WGSL remains valid (WESL is a strict superset; "Plain WGSL files with no preprocessor directives keep working" per the [migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20)), downstream shader code written for older Bevy mostly keeps working, which lowers the practical barrier the community will hit as 0.20 adoption spreads.

### Gaps
- No reliable source names any Bevy game in production using WESL shaders; this is simply too early post-0.20.
- Bevy examples repo (bevyengine/bevy_examples) WESL usage was not separately enumerated; the main-repo examples are the authoritative ones post-0.17 split anyway.

---

## Timeline (condensed, all verified above)
- **2024-05-08**: `webgpu-tools` (ex-`wgsl-tooling-wg`) org created; **2024-08-19**: wesl-rs repo created.
- **2025-02-20**: wesl 0.1.0 on crates.io; Bevy PR #17953 opened.
- **2025-03-09**: #17953 merged (experimental `.wesl` shader source).
- **2025-04 (Bevy 0.16)**: WESL ships behind `shader_format_wesl` (default off); `shader_material_wesl` example added.
- **2025-04-29**: `bevy-wgsl` package repo created.
- **2025-05-05**: Bevy "WESL-ification" working group announced.
- **2025-09-30 (0.17)** / **2026-02 (0.18)**: no WESL highlights; feature stays gated.
- **2026-06-19 (0.19)**: Bevy endorses WESL as "shader language of choice"; plans internal shader port.
- **2026-07-21 → 2026-08-05**: PR #25088 "Migrate to WESL" opened → merged; naga_oil and custom WGSL dialect removed.
- **2026-10-04**: wesl 0.6.0 released; Bevy main pins `wesl 0.6.0` with `naga-ext`.
- **2026-10-08 (0.20)**: WESL official and unconditional; wgsl-analyzer LSP support shipped; migration guide published.
