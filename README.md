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

GPUI's `runtime_shaders` feature is on, so building doesn't need a full Xcode install.
