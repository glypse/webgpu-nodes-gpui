# runtime

wgpu renderer, playback clock and player for the shader programs `shader-graph` generates. Ports `/Volumes/T7WA/GitHub/webgpu-nodes/packages/runtime`.

## Module map
- `src/lib.rs`: public surface. Everything else is private; re-export new API here.
- `src/renderer.rs`: `Renderer` (device, queue, targets, readback), `Compiler` (device handle, compiles plans from any thread), `Program`, `Frame`, `Presented`, `RenderOptions`, `PixelFormat`. Largest file.
- `src/compile.rs`: `CompileQueue`, one background "shader compile" thread, latest-plan-wins.
- `src/target.rs`: `Target`, per-canvas-size textures, simulation state and frame uniform blocks.
- `src/simulation.rs`: `next_step`/`Step`/`StepKind`: how a simulation advances, restarts or repeats; `SimulationState` holds ping-pong textures.
- `src/uniforms.rs`: `pack_block`, writes name-keyed values at the offsets a `UniformBlock` lays out.
- `src/clock.rs`: `Clock`, `Timing`, `FrameInputs`. Fixed-rate or real-time project time.
- `src/player.rs`: `Player`, drives a `Clock` from the host's animation frames; waits for GPU completion.
- `src/frame_meter.rs`, `src/refresh_rate.rs`: `FrameMeter`/`FrameRate` (playback stats), `RefreshRateMeter`.
- `src/gpu_timer.rs`: timestamp queries (optional, needs `TIMESTAMP_QUERY`).
- `src/present.rs`: `Presenter`, sRGB encode and optional premultiply into the output format.
- `src/mips.rs`: `MipGenerator`, 2x2 mean mips, one pipeline per format.
- `src/formats.rs`: shader-graph texture format and sample type to wgpu.
- `src/media.rs`, `src/media/library.rs`: `MediaLibrary`, `MediaSource`, `MediaStatus`; decoded images and video textures keyed by asset, shared by content hash.
- `src/media/video.rs`: `VideoDecoder` trait, `VideoStream` (decode thread, frame cache), `open_decoder`, `SystemVideoProbe`.
- `src/media/ffmpeg_decoder.rs`: the ffmpeg implementation of `VideoDecoder`.
- `src/surface.rs` (macOS only): `CVPixelBuffer` pool and wrapping as wgpu textures for zero-copy presentation.

## Data flow
- Input: `shader_graph::codegen::RenderPlan` (passes, resources, simulation stages) and its `ShaderProgram`s.
- `Compiler::compile` / `CompileQueue::submit` -> `Program` (one `CompiledPass` per pass: pipeline, bind group layout, uniform buffer).
- `Renderer::set_uniforms` packs named values into each pass's uniform buffer. `sync_media` takes the project's assets; `poll_media` uploads finished decodes.
- `Renderer::submit` / `submit_surfaces` (or `render` / `simulate`) -> `encode_plan` per target: resolves sizes, writes frame and frame-value blocks, runs passes, steps simulations.
- Output: `take_frame` returns `Presented::Pixels(Frame)` or `Presented::Surfaces` once the copy/draw finishes. Never blocks; `render` does block.
- Threading: `Renderer` is single-threaded (owned by the app's playback or export). `Compiler` clones the device and may compile on another thread. `CompileQueue`'s worker calls `wake` after each result. Video decoding runs on its own thread per stream; media wakers fire from decode threads.
- Shader swap-in: the app calls `CompileQueue::submit` on each edit; `try_next`/`wait_next` return `Compiled { id, result }`, and the app keeps the newest id. The app's `finish_compiling` uses `wait_next` so an export draws with the latest source.

## Invariants and gotchas
- Uniform layout is owned by shader-graph (`codegen/program.rs::layout`, WGSL rules: vec2 align 8, vec3/vec4 align 16, size rounded to 16, minimum 16). `pack_block` trusts `UniformBlock` offsets and skips fields with no value. Never hard-code offsets in runtime.
- A `UniformBlock` with `size == 0` is not bound or written.
- `Renderer::new` requests `TIMESTAMP_QUERY` only when the adapter has it (timing is optional) and the adapter's `max_texture_dimension_2d`. The device is created once, owned by `Renderer`; `Compiler` clones its handle.
- Uncaptured GPU errors are logged, not panicked (`on_uncaptured_error`).
- Linear draw format is `Rgba16Float` (`LINEAR_FORMAT`); readback is `Bgra8Unorm` or `Rgba8Unorm`. `Frame::to_png` swaps B and R for BGRA.
- Readback rows are padded to `COPY_BYTES_PER_ROW_ALIGNMENT`; `read_pixels` strips the padding.
- At most `READBACK_DEPTH` (2) frames are in flight; `submit` calls `ensure_room` and errors when full. The caller drains with `take_frame`.
- Targets are keyed by canvas `Size`. Each keeps its own simulation state and frame blocks; a size change via `Target::resized` keeps transients and blocks.
- Simulations: a time going backwards restarts them; the same frame drawn again repeats its step (`next_step`). Skipped frames fold into one step.
- `Player` holds back new frames while GPU work from the last draw is unfinished; the host must call `finished()`.
- `Clock` fixed-rate time is `frame / fps` regardless of how it was reached; real-time follows the wall clock with a cap on a single step to absorb stalls.
- Surfaces are macOS only; the `Surface` re-export and `Presented::Surfaces` are `cfg(target_os = "macos")`.

## Adding a common thing
- New per-frame value: declare it in shader-graph's frame block (`shader_graph::frame`), then add the value to the `BTreeMap` in `Renderer::write_frame_block` (or `write_frame_values` for CPU-computed frame values, via `FrameValues`).
- New user uniform: nothing to do in runtime. `set_uniforms` writes any name the block declares; shader-graph generates the field.

## Tests
- Unit tests: `cargo test -p runtime --lib`. Pure logic (clock, simulation steps, uniform packing, refresh rate, frame meter).
- Integration: `cargo test -p runtime` runs `tests/passes.rs` (multi-pass programs, simulations, blurs, frame values), `tests/media.rs` (image decode) and `tests/video.rs` (ffmpeg decode, needs the FFmpeg build).
- GPU tests call `Renderer::new()` and need a real adapter (macOS Metal in practice). They run on the device; there is no software fallback.
- Build FFmpeg first (`cargo install patch-crate && cargo patch-crate`, see root AGENTS.md) or the `ffmpeg-next` crate fails to link.
- Profiling and probing: `docs/probing.md`.

## Dependencies
- Workspace deps: `wgpu`, `anyhow`, `thiserror`, `pollster`, `log`.
- Crate deps: `shader-graph` (types, codegen, passes, assets), `rayon` (image decode pool), `half`, `image`, `ffmpeg-next` (pinned `=9.1.0`, built with zlib and software scaling), macOS only: `objc2`, `objc2-core-foundation`, `objc2-core-video`, `objc2-io-surface`, `objc2-metal`.
- Dev-dependency: `image` (png).
- Dependents: `app` (playback, the editor), `export` (`export/src/run.rs`, offline renders). `shader-graph` dev-depends on `runtime` for its tests.
