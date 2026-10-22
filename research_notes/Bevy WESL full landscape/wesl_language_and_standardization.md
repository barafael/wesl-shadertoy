# WESL Language & Compiler — Reference Notes (as of 2026-10-22)

Scope: the WESL language itself, the reference compiler(s), and the standardization relationship to W3C WGSL. Bevy/ecosystem adoption is covered by other researchers.

## Design philosophy and relationship to WGSL

### Takeaway
WESL ("WebGPU Enhanced Shading Language", pronounced 'weasel') is a community-standard, strictly-additive superset of WGSL that compiles/translates down to vanilla WGSL before reaching WebGPU; its features are deliberately designed as future WGSL proposals, and the language is versioned by "editions" (current: `2026_pre`) with an experimental→stable staging model.

### Cited Findings
- The spec defines WESL as "a modestly improved, practical variant of WGSL… WESL should feel like WGSL with a few useful things added"; name expansion: "**W**ebGPU **E**nhanced **S**hading **L**anguage, pronounced like 'weasel'" — [Spec About](https://wesl-lang.dev/spec/README.html)
- Naming is not fully consistent across artifacts: the site's llms.txt titles it "WESL (WebGPU Extended Shading Language)" ([llms.txt](https://wesl-lang.dev/llms.txt)) and the wesl-rs repo describes it as "a compiler for WESL (WGSL Extended) shaders" ([github.com/webgpu-tools/wesl-rs](https://github.com/webgpu-tools/wesl-rs)).
- "WESL is a strict superset of WGSL. WESL extensions are proposed WebGPU features that you can use today. Existing `.wgsl` files work as-is." — [wesl-lang.dev home](https://wesl-lang.dev/); llms.txt: "a fully backward-compatible strict superset of WGSL" — [llms.txt](https://wesl-lang.dev/llms.txt)
- Relationship to engines: "WESL enhancements are invisible to WebGPU engines like Dawn and wgpu. All WESL enhancements are translated to vanilla WGSL before being passed to WebGPU calls such as `createShaderModule()`" and "Most WESL enhancements are intended as proposals for future versions of W3C standard WGSL." — [Spec About](https://wesl-lang.dev/spec/README.html)
- Design guidelines (Designing.md): prioritize enhancements that are (1) important for community projects, (2) natural to WGSL programmers, (3) not too difficult to integrate into community tools; "Vanilla WGSL programmers first… We should be additive to WGSL, not change semantics"; avoid imitating Rust/TS syntax unless semantics fully match ("Aim for simple instead of imitative"); target audience is part-time shader programmers and medium-size teams — [Designing.md](https://github.com/webgpu-tools/wesl-spec/blob/main/Designing.md)
- Reserved-word strategy: "WESL features use reserved words from WGSL (e.g. `import`) so that they can serve as true proposals for WGSL adoption. WESL maintainers are technical advisors to the W3C WebGPU committee, and we design features with the goal of minimal divergence from a possible future WGSL." — [Designing.md](https://github.com/webgpu-tools/wesl-spec/blob/main/Designing.md); llms.txt repeats the technical-advisor claim — [llms.txt](https://wesl-lang.dev/llms.txt)
- File extension: WESL files use `.wesl`; `.wgsl` files are also accepted and linkers fall back to `.wgsl` when a `.wesl` file is not found, enabling file-by-file migration — [Spec About](https://wesl-lang.dev/spec/README.html), [llms.txt](https://wesl-lang.dev/llms.txt)
- Edition/versioning: current edition is `2026_pre`; `edition` is the only required field of `wesl.toml` (e.g. `edition = "2026_pre"`) — [llms.txt](https://wesl-lang.dev/llms.txt). The Versioning spec draft (still pre-1.0) says: "As we stabilize for 1.0… Feature flags or versions?… Look to Rust editions as a model for versioning" (early sketch mentioned `version WESL2024`) — [Versioning spec](https://wesl-lang.dev/spec/Versioning.html)
- Stability policy: "Stabilized features will be grouped in periodic releases with backwards compatibility with the aim of nurturing an ecosystem of WESL/WGSL libraries"; experimental features are opt-in — [Spec About](https://wesl-lang.dev/spec/README.html); before inclusion in a stable edition a feature must be "polished, proven in use, tested, and well documented" — [Designing.md](https://github.com/webgpu-tools/wesl-spec/blob/main/Designing.md)
- The wesl-rs README "Status" section (last update 2026-05) states "**WESL 0.2** was released" with imports, `@if`/`@elif`/`@else`, and cargo packages — note this "0.2" refers to the language/feature-set release, while the `wesl` crate itself is at 0.6.0 (2026-10-04) — [wesl-rs README](https://github.com/webgpu-tools/wesl-rs); [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/)

### Inferences
- The "pre" in edition `2026_pre` signals a pre-stable edition ahead of a 1.0 stable edition; the versioning doc is still an open draft, so edition naming may change before 1.0.
- The multiple name expansions ("Enhanced"/"Extended"/"WGSL Extended") suggest the acronym was retro-fitted; "WebGPU Enhanced Shading Language" from the spec repo is the authoritative one.

### Gaps
- No dated announcement of when edition `2026_pre` was introduced; the Versioning spec page is still written as an open question list.

## Feature set: shipped vs proposed, with exact syntax

### Takeaway
Shipped and stable in both linkers: `import` statements + inline qualified paths, `@if`/`@elif`/`@else` conditional translation, and npm/cargo packaging (`wesl.toml`). Specified-but-unimplemented: visibility keywords and wildcard imports. Experimental: const injection (`constants::`), virtual modules, const-expression eval/exec, and generics (`@type`). Planned only: namespaces/inline modules, automatic bindings.

### Cited Findings

**Shipped (stable, per README "WESL 0.2" status and llms.txt):**
- **Imports** — bring declarations/modules into scope; must be at the top of the file; all top-level declarations (functions, structs, vars, consts, aliases) and modules are importable. Exact syntax:
  ```wgsl
  import sdf::primitives::box;      // decl `box` from module `primitives` of package `sdf`
  import package::binds::mySampler; // app's own module (reserved prefix `package::`)
  import super::util;               // sibling module (one level up)
  import package::animals::{ bird, mammals::{ dog, cat } };  // nested braces
  ```
  — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html), [llms.txt](https://wesl-lang.dev/llms.txt). Qualified module paths are usable anywhere an identifier is, with no import statement required (e.g. `super::util::foo()`, `math::constants::PI`) — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html). Special prefixes: `package::` (app root), `super::` (parent, stackable), `constants::` (host-injected) — [llms.txt](https://wesl-lang.dev/llms.txt). Files map to module paths via `::` (`shaders/utils/helper.wesl` → `package::utils::helper`); hyphens in filenames become underscores — [llms.txt](https://wesl-lang.dev/llms.txt). Linking starts from an application root module; only transitively-referenced modules are emitted (dead-code stripping; imports are lazy) and root-module names are never mangled — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html).
- **Conditional translation** — `@if(condition)` attribute (plus `@elif`, `@else`) conditionally includes the annotated element based on host-provided feature flags at link time; condition grammar is a boolean expression of feature names with `&&`, `||`, `!`, `true`, `false`. Applicable to top-level declarations, statements, struct members, and function parameters:
  ```wgsl
  @if(debug_mode && !raytracing_disabled)
  var<private> debug_count_iterations: i32 = 0;

  struct Line {
      start_point: point, end_point: point,
      @if(colored) color: vec4f,   // struct member
  }
  @if(A) { … } @elif(B) { … } @else { … }   // chained blocks
  ```
  — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html), [llms.txt](https://wesl-lang.dev/llms.txt). Unused branches are stripped from output WGSL — [llms.txt](https://wesl-lang.dev/llms.txt). Full rules in the [Conditional Translation spec](https://wesl-lang.dev/spec/ConditionalTranslation.html).
- **Packaging** — shader libraries published to npm and cargo; module path of a package starts with the (sanitized) package name: hyphens→underscores, `@` removed, `/`→`__` (e.g. `@foobar/shader-utils` → `foobar__shader_utils::`) — [llms.txt](https://wesl-lang.dev/llms.txt). `wesl.toml` config (all fields optional except `edition`): `edition`, `root`, `include`, `exclude`, `package-manager` ("npm"|"cargo") — [llms.txt](https://wesl-lang.dev/llms.txt), [wesl.toml spec](https://wesl-lang.dev/spec/WeslToml.html). The spec's `dependencies` table is *not* implemented — wesl-js autodetects packages from `node_modules`; wesl-rs declares them in `build.rs` — [llms.txt](https://wesl-lang.dev/llms.txt).
- **Const injection** *(experimental)* — host (Rust/JS) injects shader constants available under `constants::`; numbers emit directly, strings emit verbatim; each becomes a WGSL `const` in output; works in attribute positions (`@binding(constants::slot)`) because WGSL attributes accept const-expressions — [llms.txt](https://wesl-lang.dev/llms.txt), [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html).
- **Virtual modules** *(experimental)* — host-provided functions generate WESL under a custom prefix (`virtualLibs` in JS, `VirtualResolver` in Rust) — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html).
- **Eval/exec** *(experimental, wesl-rs `eval` feature)* — CPU evaluation/execution of WESL code, early const-expression evaluation (full lowering via the `lower` compiler option), user-defined `@const` function attributes; tested against the WebGPU Conformance Test Suite via [k2d222/wesl-cts](https://github.com/k2d222/wesl-cts) — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).

**Specified but NOT implemented in shipped linkers:**
- **Visibility** — three levels: `public` (any package), *package* (default), `private` (declaring module only), e.g. `public fn dot2(v: vec2f) -> f32 { … }`; includes re-exports via `public import`. llms.txt explicitly warns: "Do not emit `public`/`private` visibility keywords or `public import` re-exports yet — the shipped linkers do not accept them." — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html), [llms.txt](https://wesl-lang.dev/llms.txt), [Visibility spec](https://wesl-lang.dev/spec/Visibility.html)
- **Wildcard imports** — `import package::utils::*;` is defined in the spec but "the shipped linkers do not accept `*` in import paths" — [llms.txt](https://wesl-lang.dev/llms.txt)

**Experimental / proposed:**
- **Generics** — experimental in wesl-rs behind the `generics` feature flag, described as "user-defined type-generators and generic functions" using **`@type` attributes** (per the wgsl-parse feature table: "generics — `@type` attributes — experimental") — [docs.rs/wesl 0.6.0 Features table](https://docs.rs/wesl/latest/wesl/), [docs.rs/wgsl-parse 0.6.0](https://docs.rs/wgsl-parse/latest/wgsl_parse/). The design lives on a spec branch ([k2d222/wesl-spec, `generics` branch, Generics.md](https://github.com/k2d222/wesl-spec/blob/generics/Generics.md), fetched 2026-10-22): it argues WGSL already has parametric mechanisms (type-generators, overloads) not exposed to user code, and proposes (a) *Generic Declarations* and (b) *Generic Modules* (files as "implicit modules" that can be specialized by overriding immutable global/alias/struct/fn declarations), formalized over a "type hierarchy" derived from WGSL conversion ranks (e.g. `u32` <: `AbstractInt`, `VecN<S>` <: `VecN<T>`).
- **Namespaces / inline modules** and **automatic bindings / structs of bindings** — listed as "planned (to be designed with the WESL team)" in the wesl-rs README status section (last update 2026-05) — [wesl-rs README](https://github.com/webgpu-tools/wesl-rs).
- **Module parameters** — design proposal document exists in the spec repo: `proposals/222-ModuleParameters.md` — [wesl-spec repo tree](https://github.com/webgpu-tools/wesl-spec)

### Inferences
- The feature pipeline in practice is: idea → experimental (feature flag in one implementation) → specified in wesl-spec → implemented in both linkers → stable edition. Visibility and wildcard imports sit at the "specified, not implemented" stage; generics at "experimental, spec still on a branch".
- Using `@type`/`@if` attribute syntax rather than `<T>` angle brackets is a deliberate consequence of the LALR(1) grammar constraint (see compiler section) — attribute-based extension syntax avoids colliding with WGSL's template-argument grammar.

### Gaps
- No dated public roadmap/milestone list with target versions for namespaces/auto-bindings in wesl-rs (README status section is undated prose, "last update: 2026-05").
- Could not verify whether the JS linker (`wesl` npm) implements const-expression lowering/validation to the same depth as wesl-rs `eval`.

## Compiler architecture

### Takeaway
There are two independent implementations — `wesl-rs` (Rust, the reference compiler, crate `wesl` 0.6.0) and `wesl-js` (TypeScript) — both structured as: parse (LALR(1) grammar written from the WGSL spec, in wesl-rs via lalrpop) → resolve/link modules → apply passes (conditional translation, mangling) → pretty-print the syntax tree back to WGSL text, with a sourcemap module mapping output back to WESL sources. Type checking is not part of linking; it lives in the experimental `eval` module.

### Cited Findings
- Crate `wesl` 0.6.0 (published 2026-10-04, owner `k2d222`, MIT/Apache-2.0) is "The WESL compiler"; high-level API: `Compiler::default().compile("path/to/shader.wesl")` → `CompileResult`, and WGSL text is obtained by stringifying the (transformed) syntax tree: `compile_result.syntax.to_string()`; `build.rs` workflow via `write_artifact`/`emit_rerun_if_changed` — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/)
- Parser: dependency `wgsl-parse` 0.6.0 is "A parser and syntax tree for WGSL files, written directly from the specification with **lalrpop**" (an LALR(1) parser generator); WESL extensions are grammar feature flags: `imports` (complete), `attributes` — extra attribute locations on statements (complete), `condcomp` — `@if` attributes (complete), `generics` — `@type` attributes (experimental), plus `naga-ext` for Naga/WGPU extensions. Syntax tree root is `TranslationUnit`; all nodes implement `FromStr` and pretty-printing `Display` — [docs.rs/wgsl-parse 0.6.0](https://docs.rs/wgsl-parse/latest/wgsl_parse/)
- Grammar lineage: k2d222: "wesl-rs uses a **LR(1) parser, like in the WGSL spec**" (and adapting it to handle a statement ambiguity would mean "no longer [being] a LR(1) grammar"); "wesl-js uses a different kind of parser that can deal with this ambiguity" — [wesl-rs issue #162](https://github.com/webgpu-tools/wesl-rs/issues/162) (Oct–Dec 2025)
- wesl crate module map (docs.rs 0.6.0): `syntax` (WGSL/WESL syntax tree, root `TranslationUnit`), `resolver` (`Resolver`/`AsyncResolver`/`Constants` — module source resolution), `pass` ("low-level reusable compilation passes", `Feature`/`Features`), `mangler` (`Mangler` trait, `ManglerKind` schemes), `sourcemap` (`SourceMap` trait + implementations), `eval` ("Typechecking, Evaluation and Validation", behind `eval` feature), `package` (`PackageBuilder`), `toml_cfg` (`wesl.toml` parsing and file scanning), `error` — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/)
- Feature flags of the `wesl` crate: `generics` (experimental), `package` (experimental), `eval` (experimental), `naga-ext` (experimental), `serde` — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/)
- Type-checking depth: plain linking is a syntactic transform (parse → link → mangle → emit); semantic type checking/evaluation exists only in the experimental `eval` module (CPU execution, const-expression evaluation, validation, `@const`), validated against the WebGPU CTS ([k2d222/wesl-cts](https://github.com/k2d222/wesl-cts)) — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/)
- Name mangling on lowering: the Rust linker mangles *all* non-root-module declarations (e.g. `helper` → `package_utils_helper`); the JS linker mangles only conflicting names; root-module/host-visible names (entry points, `override`, `@binding` vars) are never mangled — [llms.txt](https://wesl-lang.dev/llms.txt); dedicated [Name Mangling spec](https://wesl-lang.dev/spec/NameMangling.html). `enable` directives are not inherited — the root module must re-declare them — [llms.txt](https://wesl-lang.dev/llms.txt)
- Sourcemap: the `sourcemap` module supports mapping compiled WGSL back to WESL sources; the JS runtime's `linked.createShaderModule(device, …)` provides "WESL source-mapped error reporting" — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/), [llms.txt](https://wesl-lang.dev/llms.txt)
- Performance/footprint: `link()` >250K lines/sec single-threaded, ~5 ms for a 1400-line shader; `linkRegistry()` caches parsed ASTs so re-links only pay bind+emit; linkers are <20 KB (Brotli) and embeddable — [llms.txt](https://wesl-lang.dev/llms.txt), [wesl-lang.dev home](https://wesl-lang.dev/)
- Package format: `wesl.toml` (edition/root/include/exclude/package-manager) parsed by `toml_cfg`; JS packaging via `wesl-packager` producing a `weslBundle.js` npm export — [llms.txt](https://wesl-lang.dev/llms.txt), [wesl.toml spec](https://wesl-lang.dev/spec/WeslToml.html), [Packaging spec](https://wesl-lang.dev/spec/Packaging.html)
- Editor-facing grammars are separate from the compiler: `lezer-wesl` (CodeMirror) and `tree-sitter-wesl` (Neovim/Emacs/Zed) — [Internals doc](https://wesl-lang.dev/docs/Internals.html)
- The wgsl-analyzer language server (forthcoming) is "based on production rust-analyzer" techniques, "incremental and error resilient", with a "resilient parser to report multiple errors" (GDC 2026 "Modern WebGPU tools" deck, Lee Mighdoll, slides reference Mathis Brossier) — [GDC 2026 deck](https://wesl-lang.dev/gdc-2026/) (slide content extracted from the Slidev bundle 2026-10-22)

### Inferences
- The architecture is a classic transpiler pipeline (parse → passes → pretty-print), not a macro system or text preprocessor: because output is regenerated from a syntax tree, WESL can guarantee output is valid WGSL and can pretty-print canonically — unlike C-style preprocessors.
- The two implementations' parser divergence (LALR(1) in Rust vs a more permissive parser in JS) is a recurring spec-forcing pressure: the spec must be writable as an LR(1) grammar so both implementations agree, which explains the attribute-based (`@if`, `@type`) rather than bracket-based extension syntax.

### Gaps
- Exact `SourceMap` API details (granularity, whether JS and Rust share a format) were not documented in the public docs index; docs.rs reports only 40.84% of the `wesl` crate documented — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/).
- No public statement found on whether wesl-js reuses the same grammar file as wesl-rs or maintains its own grammar definition.

## Staged standardization process and adoption into WGSL

### Takeaway
WESL's documented standardization model is two-track: internally, features graduate experimental → feature-flagged → stable-edition; externally, WESL acts as an incubator whose features (built on WGSL reserved words) are pitched to the W3C WebGPU CG, whose own `proposals/` staging (Draft → Merged) now hosts a WGSL-native **namespaces** proposal merged 2026-09-16. I found no formal numbered "stage 0/1/2/3" scheme in primary sources — that specific framing could not be verified.

### Cited Findings
- WESL-side staging: experimental enhancements need no cross-tool support; "As experiments move toward stability we'll move experimental enhancements into more tools behind a feature flag"; stable editions require "polished, proven in use, tested, and well documented" — [Designing.md](https://github.com/webgpu-tools/wesl-spec/blob/main/Designing.md)
- Incubator intent: "Most WESL enhancements are intended as proposals for future versions of W3C standard WGSL" — [Spec About](https://wesl-lang.dev/spec/README.html); "WESL features use reserved words from WGSL (e.g. `import`) so that they can serve as true proposals for WGSL adoption. WESL maintainers are technical advisors to the W3C WebGPU committee" — [Designing.md](https://github.com/webgpu-tools/wesl-spec/blob/main/Designing.md); "WESL features (imports, conditional compilation) are active proposals to future WGSL… minimal migration cost when these features land in browsers" — [llms.txt](https://wesl-lang.dev/llms.txt)
- WGSL-side staging mechanism: gpuweb/gpuweb `proposals/` directory holds "WebGPU extension documents… non-normative, non-standardized draft proposals" with statuses **Merged / Draft / Inactive / Obsolete**; implementations must not expose Draft proposals — [proposals/README.md](https://github.com/gpuweb/gpuweb/blob/main/proposals/README.md)
- **Namespaces adopted into the WGSL proposal pipeline**: PR [gpuweb/gpuweb#7310](https://github.com/gpuweb/gpuweb/pull/7310) "[wgsl] Add proposal for WGSL namespaces" by `dj2` (David Reveman), created 2026-09-01, **merged 2026-09-16** (verified via GitHub API: `merged: true, merged_at: 2026-09-16T22:27:09Z`), adding `proposals/namespaces.md` (+135 lines), linked to long-standing issue #777. Proposal status: Draft. Syntax: `namespace helpers { … }`, nested `namespace a::b { }`, `using namespace test;`, reserved non-reopenable `wgsl::` namespace for builtins, reopenable namespaces, fully-qualified names for namespaced `override`s and entry points from the API — [namespaces.md](https://github.com/gpuweb/gpuweb/blob/main/proposals/namespaces.md). Note: this is a WGSL-native proposal (Google engineer), while namespaces remain "planned" in WESL's own README — the efforts are parallel, and no source I found formally links the two.
- WESL-originated WGSL discussions (filed by WESL co-lead `mighdoll`, Lee Mighdoll): gpuweb [#5140 "@if for conditions - preview"](https://github.com/gpuweb/gpuweb/issues/5140) (2025-04-01, open) — "WESL M1 includes a feature for conditional translation… Probably conditions should eventually land in WGSL too… perhaps there's something you see now that we should adjust? If so, we can tweak things to make the upcoming user trials with conditions gather more useful info"; [#5139 "extend the grammar for more attributes"](https://github.com/gpuweb/gpuweb/issues/5139) (2025-04-01, open); [#5070 "[WESL] Host-visible names that shadow predeclared names"](https://github.com/gpuweb/gpuweb/issues/5070) (2025-02-14, open). 19 gpuweb issues/PRs mention WESL in total (GitHub search, 2026-10-22).
- WGSL spec text itself contains no mention of WESL and no "staged" feature process (grep of the full [W3C WGSL spec](https://www.w3.org/TR/WGSL/), 2026-10-22); gpuweb `process/` contains only CG process docs (e.g. `RequirementsForAdditionalFunctionality.md`).
- Presentations to the WebGPU team: "2025 Update on WESL plans with the WebGPU team" and "WESL – A Pioneer Language for WebGPU" (Symposium, 2026-02-13, [video](https://youtu.be/TEBh5vMFjJA)) — [Presentations page](https://wesl-lang.dev/docs/Presentations.html)

### Inferences
- The verifiable "staged" process is: (stage 0-equivalent) WESL experimental feature flag → (1) WESL spec + both linkers + user trials → (2) gpuweb `proposals/` Draft explainer → (3) normative WGSL spec text + implementations. The assignment's "stage 0/1/2/3" numbering may come from a talk slide I could not extract (Google-Slides decks are JS-only), but no primary source with that numbering was found.
- Namespaces landing in gpuweb before imports/`@if` suggests the CG finds in-module namespacing easier to standardize than cross-module linking; WESL's `import` remains a "user trial" per #5140's framing.

### Gaps
- No numbered "stage 0/1/2/3" standardization document found in wesl-spec, gpuweb, or the WGSL spec; if it exists it is likely only in the Google-Slides decks ("2025 Update on WESL plans"), which are not machine-readable.
- No WGSL-native `import` or `@if` proposal exists yet in gpuweb `proposals/` (only the discussion issues above); no timeline for either.

## Origin and history

### Takeaway
WESL began in May 2024 in the GitHub org now named `webgpu-tools` (formerly reachable as `wgsl-tooling-wg`), started by Lee Mighdoll and Mathis Brossier (k2d222) out of community pain around WGSL shader libraries; the spec repo came first (2024-05), wesl-rs followed (2024-08), an "M1" release landed by early 2025, and by 2026 it had talks at a WebGPU symposium (Feb 2026) and GDC 2026 (Khronos).

### Cited Findings
- GitHub org `webgpu-tools` created **2024-05-08**, blog = wesl-lang.dev (GitHub API, 2026-10-22); repo `webgpu-tools/wesl-spec` created **2024-05-08**, described as "A portable and modular superset of WGSL"; `webgpu-tools/wesl-rs` created **2024-08-19**; `webgpu-tools/wesl-js` created **2024-08-20**; `webgpu-tools/wesl-playground` created **2024-09-30** (GitHub API repo search, 2026-10-22).
- `github.com/wgsl-tooling-wg` returns 404 today, but the Wayback Machine holds a 200 snapshot of that org page dated **2026-05-02** — the org existed under that name recently (verified via archive.org availability API).
- Motivation statement: "Many community projects have independently uncovered a need for WGSL enhancements like module composition, conditional compilation, and shader libraries… a shared standard can enable reusable WGSL libraries on npm and crates.io" — [Spec About](https://wesl-lang.dev/spec/README.html); wesl-rs exists to do "what [naga_oil](https://github.com/bevyengine/naga_oil) does for Bevy, but in a framework-agnostic way" — [wesl-rs README](https://github.com/webgpu-tools/wesl-rs)
- Leadership: `k2d222` (Mathis Brossier) is the crates.io owner of `wesl` and primary author of wesl-rs/wgsl-parse ([docs.rs/wesl](https://docs.rs/wesl/latest/wesl/), [docs.rs/wgsl-parse](https://docs.rs/wgsl-parse/latest/wgsl_parse/)); Lee Mighdoll (`mighdoll`) authors the WESL-side gpuweb proposals and presented the GDC 2026 deck, which credits "Mathis Brossier (in spirit)" — [GDC 2026 deck](https://wesl-lang.dev/gdc-2026/), [gpuweb#5140](https://github.com/gpuweb/gpuweb/issues/5140)
- Timeline anchors: "Introducing WESL's M1 release" talk ([slides](https://docs.google.com/presentation/d/e/2PACX-1vRxVlBmHLmdoOCG7uk8igd8XQAi1u48k-2tgjvXysUCXlNzqum5WBM7EhFx9gNznHGgFW8tjF3NOJZw/pub), [video](https://youtu.be/Na1XTKK_Mig)); M1 already included conditional translation as of gpuweb#5140 (2025-04-01); "2025 Update on WESL plans with the WebGPU team" slides; wgsl-test talk 2026-01-13; "WESL – A Pioneer Language for WebGPU" symposium talk 2026-02-13; GDC 2026 (Khronos) "Modern WebGPU tools" — [Presentations page](https://wesl-lang.dev/docs/Presentations.html)
- The spec site frames WESL as community tooling under the WebGPU community umbrella ("WebGPU community tools", Discord + GitHub org `webgpu-tools`) — [wesl-lang.dev home](https://wesl-lang.dev/)

### Inferences
- The org was almost certainly renamed from `wgsl-tooling-wg` to `webgpu-tools` (GitHub org renames free the old login and preserve creation dates; the 2024-05-08 creation date of `webgpu-tools` matches the WESL project start, and the old name 404s only recently). The name "wgsl-tooling-wg" also matches the W3C-Community-Group "working group" naming style, consistent with the tooling-first origin. This is inference, not a directly cited rename.
- Origin story: Mighdoll (npm WGSL tooling, e.g. wgsl-tools/wgsl-analyzer lineage) + Brossier (compiler engineering, ex-naga_oil-adjacent) converged in mid-2024; Bevy's naga_oil is explicitly cited as the framework-specific prior art wesl-rs generalizes.

### Gaps
- No GPU Zen talk found on the official Presentations page; if a GPU Zen 2025/2026 WESL talk exists, it is not listed at [wesl-lang.dev/docs/Presentations](https://wesl-lang.dev/docs/Presentations.html).
- The exact first public announcement (date/venue — WebGPU Discord/Slack, or CG meeting) was not found in retrievable sources.
- Wayback snapshot of the `wgsl-tooling-wg` org page did not expose its repo list/description in the archived HTML shell, so the rename chain (which repos lived there) is unconfirmed.

## Known limitations of the compiler

### Takeaway
Main documented limitations: the LALR(1)/LR(1) grammar in wesl-rs historically rejected some valid WGSL+`@if` combinations that wesl-js accepted (issue #162, fixed Dec 2025 by changing the spec grammar); several specified features (visibility, wildcard imports, `wesl.toml` dependencies) are unimplemented; generics remain an unshipped experimental feature flag; and the root-module/entry-point restriction plus non-inherited `enable` directives constrain module design.

### Cited Findings
- **LR(1) parser strictness** — [issue #162](https://github.com/webgpu-tools/wesl-rs/issues/162): `@if(TEX) out.tex = model.tex;` failed in wesl-rs ("unexpected token `.`") while wesl-js compiled it. k2d222 (2025-10-29): `@if` on assignment statements "leads to a grammar ambiguity: `@attribute (` — the parser can't tell whether `(` belongs to the attribute or to a `(x) = 0` assignment (valid WGSL); "wesl-js uses a different kind of parser that can deal with this ambiguity. But wesl-rs uses a LR(1) parser, like in the WGSL spec, that can't" (footnote: "Maybe it could, but not trivially, and it would no longer be a LR(1) grammar"). Resolution: spec commit [c4d32e8](https://github.com/webgpu-tools/wesl-spec/commit/c4d32e8f838af01b1139a707e735ae164987d4c9) "Allow `@if` on assignment and increment statements" (2025-11-29), closed as completed in PR #175 on **2025-12-07**.
- **Specified-but-unimplemented features** are a usability trap: visibility keywords and wildcard imports parse nowhere despite being in the spec ("the shipped linkers do not accept them") — [llms.txt](https://wesl-lang.dev/llms.txt); `wesl.toml` `dependencies` "format is still changing and is not implemented" — [llms.txt](https://wesl-lang.dev/llms.txt).
- **Generics unshipped**: only behind the experimental `generics` cargo feature; spec still on a personal fork branch (`k2d222/wesl-spec`, `generics` branch) — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/), [Generics.md](https://github.com/k2d222/wesl-spec/blob/generics/Generics.md); README lists generic functions as "planned (to be designed with the WESL team)" — [wesl-rs README](https://github.com/webgpu-tools/wesl-rs).
- **Root-module restriction**: host-visible elements (entry points, `override`, `@binding` vars) must live in the application root module — "Future versions of WESL will relax this restriction" — [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html).
- **`enable` directives not inherited** across modules (root must re-declare, e.g. `enable f16;`); `const_assert` inclusion is reachability-dependent — [llms.txt](https://wesl-lang.dev/llms.txt), [WESL Features doc](https://wesl-lang.dev/docs/WESL-Extensions.html).
- **Implementation divergence**: JS and Rust linkers mangle names differently (JS: conflicts only; Rust: all non-root names), so output WGSL text differs between toolchains — [llms.txt](https://wesl-lang.dev/llms.txt); and the #162 episode documents parser-class divergence between the two implementations — [issue #162](https://github.com/webgpu-tools/wesl-rs/issues/162).
- **Documentation maturity**: only 40.84% of the `wesl` 0.6.0 crate is documented on docs.rs — [docs.rs/wesl 0.6.0](https://docs.rs/wesl/latest/wesl/). wesl-rs has 53 open issues / 5 open PRs and 146 stars as of 2026-10-22 — [repo page](https://github.com/webgpu-tools/wesl-rs).
- wgsl-analyzer (the LSP that would surface errors in-editor) is still "forthcoming" — [Internals doc](https://wesl-lang.dev/docs/Internals.html), [wesl-lang.dev home](https://wesl-lang.dev/).

### Inferences
- The compiler's biggest structural limitation is self-imposed and strategic: keeping the grammar LR(1)-compatible (like the WGSL spec grammar) guarantees WESL can be proposed upstream verbatim, at the cost of rejecting some superficially-valid `@attribute`-before-statement forms until the grammar is amended (as in #162's fix).
- "WESL as shipped" is narrower than "WESL as advertised": of the headline roadmap items (generics, namespaces, auto-bindings), none are stable in either linker as of October 2026; the stable core is imports + `@if` + packaging.

### Gaps
- No formal "known limitations"/compatibility page enumerating WGSL constructs the LR(1) grammar rejects; #162 is the only concrete documented case I found. (53 open issues exist; I did not audit them all for further grammar gaps.)
- Whether `wesl-cli` (subcommands check/compile/eval/exec/package, per pre-verified context) has capabilities beyond the library API is not documented on docs.rs (no wesl-cli docs.rs page surfaced).
