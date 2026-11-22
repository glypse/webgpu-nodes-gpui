# webgpu-nodes-gpui

A minimal Rust port of [webgpu-nodes](../webgpu-nodes): a node editor for WGSL fragment shaders, built on GPUI and wgpu.

Scope: UV, Time, Value, float math and groups. Every socket is an `f32`.

## Crates

- `crates/flow` (`gpui-flow`): a controlled node-editor canvas for GPUI, in the spirit of Svelte Flow. Knows nothing about shaders.
- `crates/shader-graph`: the headless graph model (documents, groups, edits) and WGSL codegen. Groups compile to WGSL functions.
- `crates/app` (`webgpu-nodes`): the editor, rendering the generated shader offscreen with wgpu and showing it as an image.

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
