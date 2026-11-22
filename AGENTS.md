# Working on this repo (agents)

A Rust port (gpui-fast, wgpu) of the Svelte/TypeScript app at /Volumes/T7WA/GitHub/webgpu-nodes. The port is faithful: read the original's source (packages/core, packages/runtime, packages/editor, packages/theme, apps/web; never node_modules compiled JS) and match its behaviour, defaults, concepts and file format.

Crates: `flow` (xyflow-like canvas), `shader-graph` (types, nodes, graph model, zones, groups, codegen, library, catalog), `graph-math` (colour, curve, ramp maths), `runtime` (wgpu renderer, player, clock), `theme` (tokens + components), `screen` (area layout), `guard` (panic guards), `app` (the editor binary).

Each crate has its own `crates/<crate>/AGENTS.md` (module map, invariants, how to add things, test commands): read it before working in that crate instead of exploring the source, and update it when your change invalidates it.

Other agents may work in the same checkout at the same time. Stay inside the files your task gives you. For shared registries (module lists, `Cargo.toml` deps) make small additive edits and re-read the file right before each edit. If the build breaks in a file you don't own, it's probably another agent's in-progress edit: build again later rather than fixing it.

## Build
- After a fresh checkout, run `cargo install patch-crate && cargo patch-crate` once (README, "Building"): FFmpeg's patched build needs it.

## Use crates, not in-house code
Before writing any subsystem (layout, colour maths, parsing, text editing, media IO, geometry, search…), look for a reliable crate (`cargo search`, `cargo info`, docs.rs; check maintenance and downloads) and use it. Replacing code already here with a good crate is welcome. Write it yourself only when nothing suitable exists, and name the crates you considered in your report.

## Long commands and waiting
Loops of repeated waits are the most common way agents waste a session. Follow these strictly:
- Run `cargo build`/`test`/`clippy` in the foreground with a long Bash timeout (up to 600000 ms). Pipe through `tail`/`grep` so the output is short.
- If something may take longer than 10 minutes, start it with `run_in_background: true` and then do other work or end your turn: you are notified when it finishes. Never poll it with `sleep`, `Monitor` until-loops, or repeated `tail` of its log.
- Never chain `sleep` to wait. A command blocked or timed out twice in the same way means the approach is wrong: change it, don't repeat it.
- To run the app for a bounded time, use its own flags (see the app's `--help`) instead of `sleep; kill`.
- Before re-running a command, check whether its previous result already answers the question.

## Code
- Errors: no `unwrap`/`expect`/`panic!`/fallible indexing on user, file or GPU data in non-test code. Libraries use `thiserror`, the app `anyhow`; report through `Result`, log with `log::error!/warn!`, and surface user-facing failures in the UI where the original does.
- UI listeners: `cx.safe_listener(...)` (trait `guard::SafeListener`). Other callback bodies (subscriptions, `on_mouse_event`, `on_next_frame`, closures calling `entity.update`) go in `guard::guard("context", || ...)`. Wrap `Render::render`/`RenderOnce::render` bodies in `guard::render("context", || ...)` and `canvas` closures in `guard::guard`. Never call `window.request_animation_frame()` or other paint-phase-only window APIs from event handlers.
- Performance: views that change often (playback readouts) are their own cached views; a cached view only skips re-rendering if every child view that changes is cached too.
- Comments only explain why / non-obvious constraints. Never restate code, duplicate docs, hard-code facts owned by other code, or reference other modules' or the UI's behaviour; no banners repeating the file name. Fix or delete comments your change invalidates.

## Tests
- Screenshots, profiles and benchmark scenes: see `docs/probing.md`.
- Port the original's tests for what you port, plus regression tests for bugs you hit.
- Generated WGSL must validate with naga. GPU tests run on the real device.
- UI behaviour: `TestAppContext` (see `crates/flow/tests/pointer.rs`); layout can be checked with `debug_selector`/`debug_bounds`.
- Build in the shared `target/` (don't set `CARGO_TARGET_DIR`): everyone reuses the same compiled dependencies. Cargo locks it, so a "Blocking waiting for file lock" message means another agent is building; it is not a hang. Before each commit: tests for your crates, `cargo build --workspace`, `cargo clippy --workspace --all-targets` clean for your files.

## Version control
- GitButler: `but status`, then `but commit -b dev -m "<message>" <paths>` listing only your files. Never run git write commands, never push.
- Focused commits; messages describe the change in one plain sentence (plus a short body if useful).

## Report
At the end, briefly: what changed and how it was verified, what's left (and why), API changes other crates must know, edits outside your files. Send the final report exactly once; once it's delivered, stop, even if you're woken up again.
