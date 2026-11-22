# Probing the app: screenshots, profiles, benchmarks

Build once, then point the binary at a project with flags that bound the run.

```sh
cargo build --release -p webgpu-nodes
BIN=target/release/webgpu-nodes
```

`webgpu-nodes --help` lists every flag.

## Screenshot

```sh
$BIN crates/app/scenes/default.json --play --seconds 5 --screenshot target/probe/d.png
```

The PNG is the app window's own pixels, captured even when other windows cover it. Read it with the Read tool. The first run needs Screen Recording permission for the terminal (System Settings > Privacy & Security > Screen Recording); without it the capture still works in our tests but macOS logs a warning, so check the image.

`--screenshot` waits a few frames so the window shows the state the run ended in, then quits.

## Bounded runs

- `--seconds S`: quit after S seconds from launch. Replaces `sleep; kill`.
- `--frames N`: quit once N preview frames have been presented. Without `--play` the preview draws a single frame, so use `--seconds` there.
- `--play`: start playback at launch.
- `--window WxH`: window size in points.
- `--output`: open an output window beside the project window. `--screenshot` then captures the output window (the one titled "Output") instead of the project window.
- No flags: the app runs as before.

Exit status is 0 on a clean quit, 1 if `--screenshot` could not be written.

## Profile

```sh
$BIN crates/app/scenes/voronoi.wgnodes --play --seconds 10 --profile target/probe/v.json
```

This writes a Chrome trace to `v.json` (open it in <https://ui.perfetto.dev>) and prints a summary when the app quits. The trace is written by the app's quit hook, so it is complete even though gpui exits the process itself.

Reading the summary:

- `frames presented` and `fps` count preview frames (the `present` span). `fps` is measured between the first and last frame.
- `frame time` is the gap between consecutive presented frames: p50, p95, max. A large max with a normal p50 is a stall (often the first frame).
- `gpu time` is the GPU's per-frame time, from the timestamp queries the renderer already records.
- The span table lists each span's busy time: `count`, `total` in ms, per-call `mean`, `p50`, `p95`, `max`, and `of wall`, the share of the run's wall time the span was busy. Spans can overlap, so the shares need not add up to 100%.

Span names:

- `playback tick`: one playback animation frame on the CPU.
- `submit`: encoding and submitting a frame to the GPU.
- `present`: showing a finished frame in the window.
- `render <view>`: a view's render function, from `guard::render`. Shown per view name.

Not covered: gpui's layout and paint phases. gpui-fast only exposes them through `Window::layout_stats`, which is compiled only with its `test-support` feature.

## CPU sampling with samply

For where CPU time goes inside functions, use a sampling profiler. `samply` is not installed in this checkout. To install it, run `cargo install samply` (this compiles and installs a binary into `~/.cargo/bin`). Then:

```sh
samply record --save-only -o target/probe/cpu.json.gz -- \
  $BIN crates/app/scenes/voronoi.wgnodes --play --seconds 10
samply load target/probe/cpu.json.gz
```

This section has not been run; check `samply record --help` if the flags differ.

## Benchmark scenes

`crates/app/scenes/` holds the benchmark projects:

- `default.json`: the default scene.
- `voronoi.wgnodes`: a fractal Voronoi scene saved from the editor (UV, aspect, a vec3 whose z is time / 2, Fractal Voronoi, Remap, OKLCh), far heavier on the GPU than the default scene.

`cargo test -p webgpu-nodes scenes` checks that both scenes load and generate WGSL that naga validates.

## Numbers from one run (release build, 10 s, 1400x860 window, vsync-limited)

| scene | frames | fps | frame p50 / p95 / max (ms) | GPU mean / p95 (ms) | playback tick mean (ms) | submit mean (ms) |
|---|---|---|---|---|---|---|
| default.json | 621 | 59.5 | 16.67 / 16.74 / 114 | 0.27 / 0.29 | 0.89 | 0.73 |

Both scenes hold the display's refresh rate, so the frame rate cannot show a regression. Use the GPU column and the per-frame busy times to compare changes. The run also shows one stall of 100 ms or more, which was not investigated.
