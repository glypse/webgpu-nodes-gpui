# crates/export (agents)

Renders a shader graph frame by frame at a fixed timestep and encodes it to MOV or MKV with FFmpeg's built-in encoders. Spec and codec decisions: `docs/video-export.md` (its decision section overrides the H.264/AV1/openh264 proposals below it). Repo rules: root `AGENTS.md`.

## Modules
- `src/lib.rs`: public types (`Container`, `Codec`, `ProResProfile`, `DnxhrProfile`, `Depth`, `Settings`, `Scene`, `ExportError`, `Progress`), `Export` (start/progress/cancel/wait), `validate`, `video_size`, `frame_count`, `first_frame`.
- `src/run.rs`: the export thread body. Compiles the program, waits for media, simulates pre-start frames, renders, pushes to the encoder, removes the partial file on error.
- `src/encode.rs`: `Encoder` (open/push/finish). Pixel formats, encoder lookup and options, BGRA→target scaling, colour tagging.
- `tests/encode.rs`: GPU and FFmpeg round-trip tests (writes, reads back with ffmpeg-next).

## Data flow and threading
- `Export::start` (UI thread) validates settings, checks the DNxHR minimum size, then spawns thread `video-export` running `run::run`.
- Thread: `Renderer::new` (GPU) → compile → `sync_media` + `wait_for_media` → `Encoder::open` → per frame: `renderer.render` (BGRA8, top row first) → `Encoder::push` → `encoder.finish`.
- Frame n is at time `n / fps`; `first_frame` = `round(start * fps)`; `frame_count` = `round(duration * fps)`, at least 1.
- Progress and cancel are atomics in `Shared`. Cancel is checked per frame and per 64-frame simulate batch; a cancelled export returns `ExportError::Cancelled`.
- `Export::wait` maps a thread panic to `ExportError::Panicked`. Dropping `Export` does not stop the thread: call `cancel` first.
- The UI consumers are `crates/app/src/{video_export,properties,shell}.rs`.

## Gotchas
- Encoders need even sides: `video_size` rounds down to even (min 2). Callers should size with it.
- Frames arrive as BGRA; `push` copies rows (stride-aware) and converts with `sws` BILINEAR. YUV output gets `sws_setColorspaceDetails` to BT.709 and tags BT.709 limited range; RGB output is full range with IEC 61966-2-1 transfer.
- Pixel formats (`encode.rs` `format`): ProRes 422 profiles YUV422P10LE; ProRes 4444 YUV444P10LE, or YUVA444P10LE with alpha. DNxHR LB/SQ/HQ YUV422P, HQX YUV422P10LE, 444 YUV444P10LE. FFV1 8-bit BGRZ (BGRA with alpha), 16-bit GBRP16LE (GBRAP16LE with alpha). PNG RGB24/RGBA; QTRLE RGB24/ARGB.
- ProRes always uses `prores_ks` (no `prores_aw` fallback). DNxHR uses the `dnxhd` encoder with a profile option; min frame 256×120.
- Container rules live in `Codec::supports_container` and `supports_alpha`; `validate` enforces them. QTRLE is MOV-only.
- Timebase: `fps*1000` is rounded to an integer, so fractional rates are approximate in thousandths.
- Each packet gets `set_duration(1)`: the MOV muxer drops a final packet without a duration.
- `run::run` deletes the output file on any error or cancel. A dropped `Encoder` leaves an unfinished file; always call `finish`.
- Errors: `ExportError` (thiserror); FFmpeg errors become `Encode(String)` via `ff()`. No `unwrap` in non-test code, except `Export::wait`'s `unwrap_or`, which is not a panic.
- FFmpeg build: `ffmpeg-next` 9.1.0 (pinned with `=`) with `build` and `build-zlib`; `ffmpeg-sys-next` is patched by `patches/ffmpeg-sys-next+9.1.1.patch` (enables zlib in configure and links `z` on non-Windows). Run `cargo install patch-crate && cargo patch-crate` on a fresh checkout; after editing the patch, `cargo patch-crate --force`. PNG needs zlib.

## Tests
- Unit: `cargo test -p export --lib` (settings validation, frame grid, even sides).
- Integration: `cargo test -p export --test encode` (real GPU and FFmpeg read-back; every codec in every container, QTRLE-in-MKV refusal, DNxHR minimum size, progress and cancel).
- All: `cargo test -p export`.

## Dependencies
- Path: `runtime` (Renderer, media, Size), `shader-graph` (`codegen::Program`).
- Workspace: `log`, `anyhow`, `thiserror`.
- Direct: `ffmpeg-next` =9.1.0 (`codec`, `format`, `software-scaling`, `build`, `build-zlib`).
- Dependents: `crates/app` (`Cargo.toml` path dep; `video_export.rs` and `properties.rs` use `Export`, `Settings`, `Codec`, and `shell.rs` uses `OutputSettings`).
