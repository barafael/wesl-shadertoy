# Feedback & Plan — 2026-10-23

User feedback session after the "it works on web" milestone. Everything below was
observed/verified in this repo unless marked *assumption*.

## 1. Findings & observations (verified)

### F1 — Canvas does not fill the viewport
Headless probe (dpr=2, window 1280×800): canvas rect is **1280×720** while the
document viewport is **1280×657**. winit writes an inline `style="width: …px;
height: …px"` on the canvas; our stylesheet only has `min-width/min-height:
100vw/vh`, which can enlarge but never shrink that inline size. Result: canvas
too tall (clipped by `overflow: hidden`) or too short (gap), depending on
browser chrome / dpr / resize timing.
Probe method: inject a `getBoundingClientRect()` logger into the served
`index.html` and run headless chromium with `--force-device-scale-factor`.

### F2 — FPS readout reflows the toolbar
`ui.label(format!("{:.0} fps", …))` (and `t = {:.1}s`) change width as digits
change, shifting the preset combobox and everything after it. Needs fixed-width
slots.

### F3 — Selection is offset from glyphs in the Code editor
The editor paints a highlighted `LayoutJob` galley *behind* a transparent-text
`TextEdit`. The selection rects are computed from **TextEdit's own internal
galley**, not from our painted galley, so any layout difference shows up as the
selection "jumping" off the visible characters. The WGSL view doesn't have the
bug because its selection lives on the same galley that is painted.
Robust fix: egui's own code-editor pattern — implement `TextEditBuffer` over the
`LayoutJob` and hand *that* to `TextEdit`, so egui paints the colored text
itself and cursor/selection align by construction.

### F4 — Line numbers broken in the WGSL view
`show_code_view` puts the code in a `Label`, which **wraps long lines** at the
available width, while the gutter galley has one row per source line. Composed
WGSL has long lines → rows drift apart. Fix: lay out/paint the code with
wrapping disabled (horizontal scroll instead), same as the editor.

### F5 — Clippy: exactly 3 warnings
All `clippy::type_complexity`, all in the vendored
`src/render/fullscreen_material.rs` (lines ~215, ~276, ~331: the `Query` /
`ViewQuery` tuples in `queue_material_bind_groups`, `prepare_material_targets`,
`draw_materials`). Fix by extracting `type` aliases (minimal deviation from
upstream Bevy code).

### F6 — Multiplayer surface to remove
`src/net.rs` (transport, messages, diff_edit), `NetPlugin`, the `Session`
window + toolbar toggle, `ShaderDoc.version` (only consumed by `diff_edit`),
`CompileStatus.ok_count` (never displayed), the `serde` + `serde_json` deps
(used **only** by net.rs), README "Multiplayer roadmap" section + Session
mentions, module docs in main.rs.

### F7 — `.wesl` programs in the wild (research)
Surveyed the public WESL corpus (Oct 2026):

| Source | Content | Catalog verdict |
| --- | --- | --- |
| `webgpu-tools/wesl-rs` `samples/mandelbrot/*.wesl` | Mandelbrot lib + fullscreen quad | Already have a better animated Mandelbrot — skip |
| `webgpu-tools/wesl-rs` `examples/random_wgsl/shaders/noise/{perlin,value}.wesl`, `hash.wesl` | Gustavson-style permutation Perlin/value noise (MIT) | **Mine it**: better noise than our `fract(sin(dot))` hash → new preset and/or upgrade Fire/Elevated |
| `webgpu-tools/wesl-js` `packages/play-dev`, `packages/wgsl-edit/demo` | uv-gradient demos (`import env::u`) | Visually trivial, Plasma already covers it — skip |
| `webgpu-tools/wesl-js` `examples/wesl-vite|deno|unbundled` | Same Mandelbrot demo, multi-file | skip |
| `webgpu-tools/wesl-js` `examples/lygia-*` | lygia library usage (polka dots) | Library demos, not standalone — skip |
| `webgpu-tools/wesl-js` `packages/wesl-bench/wesl-examples/*.wgsl` | Real-world WGSL (Bevy deferred lighting, Unity FS) | Material/compute shaders, not demos — skip |
| Bevy `assets/shaders/custom_material.wesl` | Polka-dot mesh material | Not fullscreen — skip |

**Conclusion:** nothing is worth adding verbatim — the corpus is libraries,
test fixtures, and gradient/Mandelbrot demos. Two actionable outcomes:
1. Port the Gustavson Perlin noise into a new preset (e.g. "Perlin Flow" /
   domain-warp fbm) — genuinely better visuals than our hash noise.
2. *Stretch idea:* bundle a small `package::noise` library module in our
   `VirtualResolver` so editor code can `import package::noise::perlin_noise2;`
   — showcases WESL's actual killer feature (imports) in the playground.
   Decide during implementation; keep out of the critical path.

### F8 — Code-review targets (modern Bevy 0.20, spotted during recon)
- `update_uniforms`: `Query<&Window>.iter().next()` → `Single<&Window, With<PrimaryWindow>>`.
- `RunRequest(bool)` resource-flag: fine, but consider Bevy 0.20 `Message` for
  one-shot intents; decide in review and be consistent.
- Persistence lives in `thread_local!` + egui data store → move to a small
  `Resource` (testable, no hidden state).
- `ShaderDoc.version` / `ok_count` dead fields (see F6).
- String churn: `compile_and_apply` clones the WGSL twice; `setup` clones
  presets — minor, fix opportunistically.
- Vendored `fullscreen_material.rs`: keep a header note of the upstream commit
  so future Bevy bumps don't silently drift.
- Feature-set audit after net removal (serde gone; web-sys features for File).

## 2. Instructions from the user (the ask list)

1. Fill the entire viewport. (F1)
2. FPS view must not push the remaining UI around. (F2)
3. Fix selection offset in the source editor. (F3)
4. Fix line numbers in the WGSL view. (F4)
5. Run button label → "Compile WESL Shader".
6. `cargo clippy`: fix the 3 remaining issues (simplify systems or extract
   type definitions). (F5)
7. Careful code review of the whole project, modern Bevy practices. (F8)
8. Style egui *slightly* in **gruvbox light/dark**, **no rounded corners**.
9. Pick a **random preset each start** (when there is no saved document).
10. Add **Ctrl+S** and **Ctrl+O**.
11. Research `.wesl` programs online for the catalog. (F7 — done, see verdict)
12. Add a **settings screen** and an **about screen**.
13. Remove **all hints of multiplayer-readiness**. (F6)

### Assumptions (correct me if wrong)
- **Ctrl+S** = save now: web → localStorage immediately (bypasses the 0.5 s
  debounce) + "saved ✓" status; native → write `shader.wesl` next to the
  binary. (Not a browser "Save As" download.)
- **Ctrl+O** = open a `.wesl`/`.wgsl` file: web → hidden `<input type=file>`
  via web-sys; native → status message "file picker is web-only" (a native
  dialog would need a new dep, e.g. rfd — skipped unless you want it).
- Gruvbox: both palettes shipped, default follows the OS theme, toggle lives
  in the settings screen.
- "Random preset each start" applies only when there is no restored
  localStorage document (a saved doc still wins).

## 3. Plan

Ordered so each phase lands green on its own; commits per phase.

### Phase 1 — UI fixes (F1–F4 + label)
1. Viewport: `Window { fit_canvas_to_parent: true, .. }` + CSS
   `canvas { position: fixed; inset: 0; width: 100vw; height: 100vh; }`
   (add `!important` only if winit's inline style still wins). Verify with the
   probe at dpr 1 and 2: canvas rect == viewport rect.
2. Toolbar: fixed-width slots for `t = …s` and `… fps` (`ui.add_sized`), so
   nothing shifts.
3. Run button → "Compile WESL Shader" (tooltip keeps the Ctrl+Enter hint).
4. WGSL view: no-wrap code galley painted in a both-axis `ScrollArea`, gutter
   rows stay aligned.
5. Editor: rewrite `show_code_editor` on the `TextEditBuffer`-over-`LayoutJob`
   pattern (egui code-editor style). Selection/caret align by construction;
   keep gutter, Tab/Shift-Tab, Ctrl+Enter.

### Phase 2 — Housekeeping (F5, F6)
6. Clippy: extract `type` aliases for the 3 complex queries in
   `fullscreen_material.rs`; `cargo clippy --all-targets` → 0 warnings.
7. Multiplayer removal: delete `net.rs`, `NetPlugin`, Session window/toggle,
   `version`/`ok_count` fields, serde deps; scrub README + docs.

### Phase 3 — Features (8–12)
8. Random preset on start (no saved doc): tiny xorshift seeded from
   `now_epoch_secs()`, no new deps.
9. Ctrl+S / Ctrl+O per the assumptions above (web-sys `File`/`FileList`
   features; native fallbacks as stated).
10. Gruvbox styling: custom `Visuals` for dark + light (official gruvbox hex:
    bg `#282828`/`#fbf1c7`, fg `#ebdbb2`/`#3c3836`, accents red/green/yellow/
    blue/aqua/purple/orange), `rounding = 0` on windows/buttons/frames,
    gruvbox selection color; `highlight.rs` gets gruvbox syntax palettes for
    both themes (keyword red, type aqua, string green, number purple, comment
    gray, function yellow).
11. Settings window: theme (dark/light/follow-system), random-preset-on-start
    toggle; persisted to localStorage alongside the document.
12. About window: what this is, tech stack + versions (Bevy/WESL/naga/egui),
    repo + wesl-lang.dev links, license, credits for the ported classics.

### Phase 4 — Catalog (F7)
13. New preset "Perlin Flow" (or similar) built on the ported Gustavson Perlin
    + fbm domain warp; compile-checked by `all_presets_compile`.
14. (Stretch, only if time allows) bundled `package::noise` module in the
    resolver + a preset that imports it.

### Phase 5 — Code review pass (F8)
15. Full read-through applying modern-Bevy fixes (Single queries, resource
    hygiene, message-vs-flag decision, string churn, vendored-file provenance
    header), README/docs kept in sync.

### Phase 6 — Verify & ship
16. `cargo test` (all presets still compile), `cargo clippy --all-targets`
    clean, `cargo fmt --check`, native smoke run.
17. `trunk build --release --public-url "/wesl-shadertoy/"` + headless probe:
    no panic, "shader compiled and applied", canvas rect == viewport at dpr
    1 and 2.
18. Commit per phase, push, confirm Pages workflow goes green.
