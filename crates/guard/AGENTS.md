# guard crate

Panic boundaries for gpui callbacks, so one panic doesn't abort the editor. The root AGENTS.md covers when to use each helper; this covers what they do.

- `install_panic_hook`: replaces the default stderr hook. Logs `panic at <file:line>: <message>` with a backtrace via `log::error!`. The backtrace is only filled when `RUST_BACKTRACE` is set. Called once from `crates/app/src/main.rs`.
- `guard(context, f)`: `catch_unwind` around `f`. On panic it logs `recovered from a panic in <context>`, increments the counter read by `recovered()`, and returns `None`. Callers must handle `None`.
- `guard` does not roll back: state `f` changed before panicking stays changed (`AssertUnwindSafe`).
- `SafeListener::safe_listener`: the context string is `std::any::type_name::<T>()`, so logs name the view type, not a label.
- `render(context, f)`: runs `f` under `guard`, inside a `tracing` info span `render` with `view = context`. On panic it draws a red "failed to render; see the log" placeholder for that frame only; the next frame retries.

Gotchas
- Works only while panics unwind. The workspace doesn't set `panic = "abort"`; keep it that way or these helpers do nothing.
- Each `render` call is its own boundary, so a panicking child view shows its placeholder without taking down its parents.
- Panics inside `install_panic_hook`'s own code, or on threads that don't go through `guard`, are not caught here.
- The `recovered()` counter is process-wide and only counts `guard` calls; `render` and `safe_listener` count through `guard`.
