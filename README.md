# webgpu-nodes-gpui

A node editor for WGSL fragment shaders, built on GPUI and wgpu.

## Crates

- `crates/flow` (`gpui-flow`): a controlled node-editor canvas for GPUI. Knows nothing about shaders.
- `crates/shader-graph`: the headless graph model (types, nodes, documents, zones, groups, edits, library, catalog) and WGSL codegen.
- `crates/graph-math`: colour, curve and colour-ramp maths shared by the graph's CPU evaluation and the editor widgets.
- `crates/runtime`: the wgpu renderer, playback clock, player and media decoding.
- `crates/export`: video export through FFmpeg's built-in encoders.
- `crates/theme`: design tokens and UI components.
- `crates/screen`: the editor's tiled area layout.
- `crates/guard`: panic guards for UI callbacks.
- `crates/app` (`webgpu-nodes`): the editor.

## Running

```bash
cargo run -p webgpu-nodes
```

## Building

The video export needs FFmpeg with zlib (for PNG), built from source. Its build script ignores the zlib feature, so `ffmpeg-sys-next` is patched from `patches/`. After a fresh checkout, once:

```bash
cargo install patch-crate
cargo patch-crate
```

This copies `ffmpeg-sys-next` to `target/patch/` with the patch applied. Until it has run, cargo fails to load the `[patch.crates-io]` path, and the first build compiles FFmpeg (several minutes). Re-run `cargo patch-crate --force` after changing a patch file.
