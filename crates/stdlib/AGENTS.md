# stdlib agent notes

Describes the standard library's groups in Rust and generates `crates/shader-graph/src/library/*.json` from them. The JSON is only an output: never edit a described group's file by hand.

## Modules
- `src/builder/`: the describing API. `group.rs` (`GroupBuilder`, `NodeBuilder`, `Switch`, `Frame`, `CustomWgsl`, repeat zones; all methods take `&self`, state is in a `RefCell`), `interface.rs` (`Input`, `Output`, `Variant`, `TypeVar`, `EnumRef`), `operand.rs` (`Ref`, `Operand`, `Operands`: refs are `Copy` indices into the builder), `spec.rs` (`GroupSpec`), `ids.rs` (`stable_id`, `handle_id`), `error.rs` (`ErrorKind`, `BuildError`).
- `src/build.rs`: `build_groups(&[GroupSpec])` builds every group, dependencies first, and gives each its ref (`id@version`, from `shader_graph::library::version_of`).
- `src/groups/`: the described groups, `all()` lists them.
- `src/json.rs`: the file's text (tabs, scalar arrays on one line, whole floats as integers) and its value; `unplaced` zeroes node positions.
- `src/files.rs`: reading and writing library files, listing a new file in `library.rs`' `SOURCES`, and `update_refs` (rewrites refs to rebuilt groups in files that aren't described, until none moves, and in `crates/app/scenes/*.json`).
- `src/validate.rs`: compiles a node of each group for every combination of enum values and type variables and validates the WGSL with naga.
- `src/layout.rs`: `layout_headless` (estimated node sizes, `GraphModel::layout_document`) and `layout_measured` (runs the editor's `--layout-project`).
- `src/main.rs`: the `stdlib` command.
- `tests/builder.rs`: the builder, with throwaway groups; `tests/descriptions.rs`: fails when a file drifted from its description (node positions aside).

## Describing a group
```rust
GroupSpec::new("std-component-sum", "Component Sum", "vector", &["vector"], &["sum"], |g| {
    let t = g.type_variable("t", "T", &[WgslType::Vec3f, WgslType::F32]);
    let vector = g.input("vector", Input::new(&t).label("Vector"));
    let dot = g.op("dot", (vector, 1));
    let sum = g.width_switch(vector, (vector, dot, dot, dot));
    g.output("sum", sum, Output::new(WgslType::F32).label("Sum"));
})
.aliases(&["Sum Components"])
.helper(&[])
```
- `inputs`/`outputs` are the keys `build` adds (any order); a mismatch is an error. Callers feed and read a group by these keys: `g.call("std-x").input("key", v).add().out("key")`.
- Nodes: `g.op(type, operands)` feeds inputs in the descriptor's order (a variadic last input takes the rest; a number becomes a stored value, a number for a variadic input a Declare node). `g.node(type)` adds `.input(name, ..)`, `.setting(key, option)`, `.typed(family, ty)`, `.label(..)`. A `Node` converts to its first output; `node.out("handle")` picks another.
- Own methods for node types needing more than operands: `vec`, `constant`, `width_switch(_as)`, `match_on` + `.case`, `index_switch` + `.case`, `custom_wgsl`, `repeat`, `frame`, `reroute`, `wireless`, `call`, `enum_input` (`Input::enumeration`/`toggle`).
- Everything is checked against the registry when added (node type, input and output ids, settings and options, type families, operand counts); mistakes are collected and reported together by the build, never panics.
- Ids are hashed from the group id and the order things are added, so building twice gives identical files; reordering statements changes ids and so versions.
- Each output linked to several inputs is split at a reroute, as the editor's layout does.

## Running
- `cargo run -p stdlib`: rebuilds the groups whose file drifted, and the ones named (`cargo run -p stdlib -- std-max-component`), or all with `--all`. It validates the WGSL, lays the groups out, writes the files, and rewrites refs to rebuilt groups elsewhere (shader-graph must be rebuilt afterwards: the files are embedded).
- `--dry` validates and reports what would change, writing nothing.
- `--measured [--app PATH]` takes node positions from the editor's `--layout-project` (see `docs/probing.md` for building it) instead of estimating them; `--app` names a built editor, else it's run with `cargo run -p webgpu-nodes --release`.
- `cargo test -p stdlib`.

## Gotchas
- A described group's version is the hash of its definition without positions; the stored positions only change when the group is rebuilt.
- Groups that aren't described yet keep their hand-written files; `update_refs` follows their refs to rebuilt groups.
- A group can only call groups that are described (by id).
- Adding a zone other than repeat means a method like `GroupBuilder::repeat` (its boundary sockets come from `shader_graph::zones`).

## Dependencies
- Depends on `shader-graph` (registry, document types, layout, library versions), `naga`, `clap`, `sha2`, `uuid`, `tempfile`, `anyhow`, `thiserror`.
