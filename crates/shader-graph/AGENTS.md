# shader-graph agent notes

Headless shader graph: document model, type inference, CPU evaluation and WGSL codegen. No GPU or UI.
Ports `/Volumes/T7WA/GitHub/webgpu-nodes/packages/core/src` (the TS module layout mirrors this crate).

## Modules
- `src/lib.rs`: declares all modules; re-exports `ShaderProgram`, `UniformBlock/Field/Scalar/Value` from codegen.
- `src/graph/`: document model. `document.rs` (serde file format: `GraphProject`, `GraphDocument`, `GraphNode`, ids, `ROOT_ID`), `model.rs` (`GraphModel` = project + selection/group view + history + rules), `model_*.rs` (edit rules by area: groups, zones, textures, assets, custom WGSL, frames, interface, preview, edits), `validate.rs`, `hydrate.rs` (fills defaults on load), `history.rs` (undo), `frames.rs`, `settings.rs` (render settings), `limits.rs` (caps on what a file may request), `import_groups.rs` (group bundles).
- `src/types/`: `values.rs` (WGSL scalar/vector types), `descriptors.rs` (`NodeDescriptor`, socket defs, `TypeFamily`, widgets), `node_data.rs` (`NodeData`, `InputValue`), `resolver.rs` (`TypeResolver`: type inference over links, forced types, type variables).
- `src/nodes/`: node registry. `mod.rs` (`REGISTRY`, `registered`, `resolve_descriptor`, `DYNAMIC_NODE_TYPES`), `standard.rs` (fixed-socket math/vector/logic/converter nodes), `builders.rs` (shared descriptor shapes), `dynamic.rs` (sockets depend on node/document), `cpu.rs` (CPU implementations), `texture.rs`, `media.rs`, `render_texture.rs`, `color_ramp.rs`, `curve.rs`, `custom.rs`, `dev.rs`.
- `src/codegen/`: WGSL generation. `generate.rs` (`ShaderOptions`, `generate_render_plan`/`generate_program`, function/loop emission), `plan.rs` (`RenderPlan`: passes in order), `program.rs` (`ShaderProgram`, uniform block layout), `target.rs` (what a shader draws: socket value, simulation state, render-texture items), `texture_read.rs`, `literals.rs`, `helpers.rs`.
- `src/passes/`: textures and passes a frame needs. `analysis.rs`, `textures.rs` (formats, sizes), `sources.rs` (resolves texture sockets to images), `simulation.rs` (zone state between frames), `item_size.rs`.
- `src/frame/`: frame-tier values computed on the CPU once per frame (`analysis.rs`, `program.rs`).
- `src/zones.rs`: geometry-nodes-style zones (repeat, simulation, pass repeat); output node owns `data.items`, input node points at it via `data.paired_output`.
- `src/enums.rs`: enums on group inputs; Match and Width Switch branch on them at compile time.
- `src/evaluate.rs`: CPU evaluation of a graph (`evaluate`, `evaluate_with`) for nodes with a `cpu` fn.
- `src/library.rs`: standard library groups, embedded from `src/library/*.json` via `include_str!` (`SOURCES`), loaded lazily (`LIBRARY`); `load_error()` reports failures.
- `src/library/*.json`: group definitions, one file per group.
- `src/catalog.rs`: add-menu entries (nodes, groups, zones, links) and search.
- `src/assets/`: project asset metadata (`asset.rs`), bundles beside project JSON (`bundle.rs`), import description (`import.rs`).
- `src/layout/`: auto-layout via ELK. `compute.rs` (`compute_layout`), `elk.rs` (the only call into `elk-layout`), `geometry.rs` (node sizes and handle offsets), `branches.rs` (splits branching outputs at reroutes).
- `src/color/`, `src/curve.rs`, `src/ramp.rs`: thin wrappers over `graph-math` plus widget descriptions.
- `src/testing.rs` (cfg(test)): `TestGraph` builder and `validate_wgsl` (naga).
- `src/tests/` (cfg(test)): unit tests, one file per area. `tests/gpu_color.rs`: integration test on a real GPU.

## Data flow
- Load: JSON -> `graph::GraphProject` (hydrated, validated). Edits go through `GraphModel` methods, which keep invariants and push history.
- Types: `TypeResolver::resolve(...)` over the project -> per-node input/output types (cached).
- Descriptors: `nodes::resolve_descriptor(node, project)` gives the `NodeDescriptor` for any node, including group, zone, Match, Custom WGSL and Render Texture nodes, which are built from their node/document rather than registered.
- Codegen: `codegen::generate_program(project, resolver, &ShaderOptions)` -> `Program { program, uniforms, line_map, plan }`. `plan` is the pass list; the last pass draws the frame. `line_map` maps node ids to lines of the generated code.
- CPU: `evaluate(model, node, handle)` uses each descriptor's `cpu` fn.

## Invariants and gotchas
- `nodes::REGISTRY` order is the add-menu order. Registration is by `type_id`; descriptor `cpu` falls back to `cpu::standard_cpu(type_id)` when unset.
- `DYNAMIC_NODE_TYPES` must list every type `resolve_descriptor` matches specially; `is_known_node_type` uses it together with the registry.
- `resolve_descriptor` falls back to the `declare` descriptor for unknown types and `expect`s it exists; don't remove `declare`.
- Match is registered with `type_family = None` so the add menu doesn't offer one per type.
- Dev nodes are registered always but hidden from the add menu unless developer tools are on (`dev` flag).
- CPU results must round f32 and wrap integers as WGSL does (per the `cpu` field's doc). Nodes that read pixels, textures or custom code have no `cpu`.
- `ShaderOptions::uniform_scopes` decides which unlinked input values are read from uniforms instead of written as literals; `hoist_frame_values` reads the root's frame values from a block the runtime writes once a frame.
- Zone item handle ids are shared between the input and output nodes of a zone.
- Layout: ELK is pinned exactly (`elk-layout = "=0.12.2"`) because placement changes between releases. `serde_json` has `float_roundtrip` so saved floats round-trip exactly.
- Format: serde names (`camelCase`, `tag = "kind"`) define the saved file. Changing them breaks existing projects; `hydrate.rs` fills defaults for fields missing from older files.
- `limits.rs` caps what a file can ask for; keep new expensive features under those caps.
- Node ids, edge ids and definitions are UUID v4 (`document::new_id`).

## Adding a common thing
- New node type with fixed sockets: a builder/descriptor in `src/nodes/standard.rs` (or a new file), register it in `REGISTRY` in `src/nodes/mod.rs`; add a CPU fn in `src/nodes/cpu.rs` if it has a CPU meaning; the WGSL emitter is the descriptor's `wgsl` field. The catalog picks it up from the registry.
- Node whose sockets depend on the document: add a builder in `src/nodes/dynamic.rs`, a match arm in `resolve_descriptor`, and its type in `DYNAMIC_NODE_TYPES`.
- New library group: add `src/library/<id>.json` and an `include_str!` entry in `SOURCES` in `src/library.rs`.
- New edit operation: a method on `GraphModel` in the matching `src/graph/model_*.rs`, with history recorded like its neighbours.

## Tests
- Unit tests: `cargo test -p shader-graph` (includes `src/tests/*` and per-module `tests.rs`, e.g. `src/color/tests.rs`, `src/nodes/color_ramp/tests.rs`).
- Generated WGSL is validated with naga via `testing::validate_wgsl`; `src/tests/nodes.rs` iterates `registered_nodes()`, so every registered node must generate valid WGSL for every input type.
- GPU test: `cargo test -p shader-graph --test gpu_color` needs a real wgpu device (it renders through `runtime::Renderer`).
- Test helper for graphs: `testing::TestGraph` (`new`, `add`, `set`, `output()`, and generation helpers).

## Dependencies
- Depends on: `graph-math` (colour/curve/ramp maths), `serde`, `serde_json`, `thiserror`, `sha2`, `image`, `zip`, `uuid`, `elk-layout`. Dev: `naga`, `runtime`.
- Depended on by: `runtime` (normal dep, so changes to `codegen` output or uniform layout reach the renderer), `app`, `export`.
