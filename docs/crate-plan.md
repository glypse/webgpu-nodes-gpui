# Crate plan: what to take from crates.io, what to keep

Survey of `crates/*` (Rust port) against `packages/*` and `apps/web` (original, Svelte/TS). Metadata from crates.io on 2026-11-21. Line counts are `wc -l` of the Rust files, or of the TS equivalents where noted.

## Verified

- **gpui-component 0.10.0 builds against gpui-fast 0.1.17.** It has a `gpui-fast` feature (`extern crate gpui_fast as gpui`). A scratch app (`target/agents/audit/scratch`) with `gpui_component::init`, a `Root`, and a primary `Button` compiled and linked with `cargo build` (exit 0). It was not run, so runtime behaviour is untested.
  - Caveat: gpui-component and gpui-base still pull `gpui-pre` 0.3.12 unconditionally. Expect a second gpui in the dependency graph (compile time, and binary size if it is not dead-stripped).
  - Use `default-features = false, features = ["gpui-fast"]`.
- **Other gpui node-graph crates do not fit.** gpui-node-graph (0.1.0) needs gpui 1.16.1, ferrum-flow (0.3.1) needs gpui 0.2.2, gpui-graph (0.2.2) needs gpui-pre 0.3.12. None is gpui-fast.

## Theme and widgets

gpui-component 0.10 has: Button (+ButtonGroup, Toggle), Input, NumberInput, Textarea, Input editor with highlighter, Select, Combobox, Dialog, AlertDialog, Sheet, Popover, PopupMenu, DropdownMenu, ContextMenu, Checkbox, Radio, Switch, Slider, Tooltip, Kbd, Progress, Separator, Tab, Resizable, Dock, Sidebar, ColorPicker (simple), Notification, Form and Setting, Tree, Table, VirtualList, Icon (Lucide via gpui-kit-assets), and a shadcn-style `ThemeColor` (accent, popover, border, ring, ...).

| Ours (lines) | gpui-component equivalent | Recommendation | Notes / effort |
|---|---|---|---|
| `button.rs` (231) | `button::Button` | Replace now | Small. Map `ButtonVariant`/`ButtonSize` onto its variants. |
| `kbd.rs` (51) | `kbd::Kbd` | Replace now | Trivial. |
| `separator.rs` (31) | `separator` | Replace now | Trivial. |
| `progress.rs` (93) | `progress` | Replace now | Small. |
| `tooltip.rs` (45) | `tooltip` | Replace now | Small. |
| `checkbox.rs` (199) | `checkbox::Checkbox` | Replace now | Check indeterminate state support. |
| `radio_group.rs` (184) | `radio` | Replace now | Check keyboard behaviour. |
| `toggle_group.rs` (106) | `button::ButtonGroup` / toggle | Replace now | Segmented control. |
| `popover.rs` (118) | `popover::Popover` | Replace now | Drives the add menu and pickers. |
| `menu.rs` (410) | `menu::PopupMenu` / `DropdownMenu` | Replace with the popover | Check submenu and keyboard parity with `packages/theme` menu. |
| `dialog.rs` (379) | `dialog::Dialog`, `sheet::Sheet` | Replace | Our `DialogSide` sheet mode maps to `Sheet`. |
| `select.rs` (473) | `select::Select`, `combobox` | Replace | Verify the "reopen after close" press rule we encode. |
| `input.rs` (695) + `number_field.rs` (340) | `input::Input`, `input::NumberInput` | Replace | Scrubbing (drag to change a number) is ours; keep that layer on top of NumberInput. Needs TestAppContext tests. |
| `textarea.rs` (1007) | `input::Textarea` / `input::Editor` | Replace | Largest saving. Editor has a highlighter; useful for the code view. |
| `color_picker.rs` (555) | `color_picker::ColorPicker` | Keep for now | gpui-component's picker is simple. Ours has OKLCH modes; re-evaluate. |
| `color_wheels.rs` (760), `gamut_image.rs` (220) | none | Keep | Domain-specific. |
| `curve_editor.rs` (1263) | none | Keep | Interaction is ours; maths in `graph-math` (see below). |
| `ramp_editor.rs` (923) | none | Keep | Same. |
| `icon.rs` (118) + `assets/icons` | `gpui-kit-assets` (Lucide) | Decide | Original uses phosphor-svelte. Keep Phosphor SVGs for fidelity, or switch to Lucide and accept icon differences. |
| `theme.rs` (180), `color.rs` (60) | `theme::ThemeColor` | Keep the token struct, map it | Our shadcn tokens map by name. Socket colours (`value_f32`) and other app tokens stay in a custom global. |

Net: about 2,400 lines of our widgets (button through textarea, excluding the picker, wheels, curve and ramp editors) would be replaced. Expect 2 to 4 weeks, mostly behaviour parity and tests.

## Node auto-layout (ELK)

The original uses elkjs (`^0.12.0`) in `packages/core/src/layout/layout.ts` (404 lines). Nothing in `crates/` yet.

| Crate | Version / updated | Downloads (total / recent) | License | Notes |
|---|---|---|---|---|
| `elk-layout` | 0.12.2 / 2026-11-03 | 469 / 469 | EPL-2.0 OR GPL-3.0-or-later | Pure-Rust port of ELK 0.12 (openedges/elk-rs). Exposes `layout_json`, the same JSON API as elkjs. Claims parity on 1,988 of 1,989 models. **Recommended.** |
| `elkrs` | 0.1.1 / 2026-06-17 | 1,767 / 1,597 | Apache-2.0 | Byte-exact port of ELK 0.11.0. Older ELK than the original's elkjs 0.12. Fallback. |
| `ferrigura-elk` | 0.8.1 / 2026-11-21 | 2,325 | EPL-2.0 | Layered only, pinned to elkjs 0.9.3. Not a match for 0.12. |
| `dugong` / `dagre` | 0.10.0 / 0.1.2 | 318k / 11k | MIT/Apache | Dagre (no ports). Would lose port-side layout. Not recommended. |
| `rust-sugiyama`, `layout-rs`, `graphlay` | various | 75k, 951k, 1.7k | MIT | Generic Sugiyama without ELK's port handling. Not recommended. |

Recommendation: **use for a future feature** (auto-layout), not now. The `elk-layout` licence is dual EPL-2.0 or GPL-3.0-or-later. The project is AGPL-3.0-or-later, so the GPL option is compatible, but this needs a check by whoever owns licensing. Downloads are very low, so pin the exact version and keep the layout behind a trait. Effort: small (the crate takes the elkjs JSON shape; ~400 lines of our code become a conversion step).

## Media encode and decode

The original exports video via mediabunny (WebCodecs, MP4/WebM) and imports via `runtime/src/media.ts` (630 lines). The Rust side has `image` only (`crates/runtime/src/media.rs` 106 lines, `crates/shader-graph/src/nodes/media.rs` 97). No video export yet.

| Option | Crate(s) and version | Downloads | License | Platform | Fit |
|---|---|---|---|---|---|
| AVFoundation via objc2 | `objc2-av-foundation` 0.3.2, `objc2-video-toolbox` 0.3.2, `objc2-core-media` 0.3.2 | 3.0M / 350k / 3.5M | MIT | macOS only | Hardware H.264/HEVC, AVAssetWriter for MP4/MOV, AVAssetReader for import. Already on objc2 in `runtime`. **Best for macOS-first.** |
| FFmpeg bindings | `ffmpeg-next` 9.1.0 (2026-11-11); `video-rs` 0.13.0 | 9.2M / 517k | MIT (FFmpeg is LGPL/GPL, build-dependent) | system FFmpeg libs | Broadest codecs and containers. Needs a system dependency. **Best cross-platform fallback.** |
| `openh264` | 0.9.10 (2026-11-10), `openh264-sys2` | 2.4M | BSD-2-Clause | builds Cisco's C source (`source` feature) | H.264 encode and decode, no system dependency. Needs the H.264 licence position checked for distribution. |
| `muxide` | 0.5.0 (2026-10-19) | 44k | MIT OR Apache-2.0 | pure Rust | MP4 muxer for already-encoded H.264. Pairs with openh264. Young. |
| `mp4-atom` | 0.17.1 (2026-11-04) | 742k | MIT OR Apache-2.0 | pure Rust | Lower-level MP4 box reader/writer (kixelated). Alternative muxer. |
| `mp4` (alfg) | 0.14.0 (2023-08) | 16M | MIT | pure Rust | Stale. Not recommended. |
| `re_mp4` | 0.5.1 (2026-07-08) | 2.7M | MIT OR Apache-2.0 | pure Rust | Demux. Maintained by Rerun. |
| `rav1e` | 0.8.1 (2025-06-16) | 63M | BSD-2-Clause | pure Rust | AV1 encode. Slow. Only if AV1 is wanted. |
| `symphonia` | 0.6.1 (2026-08-13) | 23M | MPL-2.0 | pure Rust | Audio decode for import, if audio is needed. |
| `gif` / `gifski` | 0.14.2 / 1.34.0 | 185M / 707k | MIT OR Apache-2.0 / AGPL-3.0-or-later | pure Rust | GIF encode via `gifski` (best quality, AGPL matches the project). `gif` for plain encode. |
| `image` | 0.25.10 (already used) | 261M | MIT OR Apache-2.0 | pure Rust | Keep for PNG/JPEG/EXR decode. |

Recommendation: **future feature**. Add a `VideoEncoder` trait in `runtime`. Implement it with AVFoundation on macOS first, with `ffmpeg-next` behind a cargo feature as the cross-platform path. Use `gifski` for GIF export. Do not add `mp4` (stale). Risks: AVFoundation is macOS-only; FFmpeg adds a system dependency and LGPL/GPL build choices; H.264 patent and licence terms for distributed binaries need a decision. Effort: medium to large per encoder.

## Colour maths

`crates/graph-math/src/color/` (convert 494, display 97, modes 121, mod 25 = 737 lines) and `theme/src/color.rs` (60).

| Crate | Version / updated | Downloads | License | Covers |
|---|---|---|---|---|
| `palette` | 0.7.7 / 2026-08-02 | 44M | MIT OR Apache-2.0 | sRGB, linear, Lab, Lch, Oklab, Oklch, HSV, HSL, Yxy, typed conversions. No Display P3, no gamut mapping. |
| `color` (linebender) | 0.3.3 / 2026-05-05 | 15M | Apache-2.0 OR MIT | CSS Color 4 parse and serialise (incl. `oklch()`, `color(display-p3 …)`), Display P3, Rec2020, ProPhoto. Gradients. No gamut mapping. |
| `csscolorparser` | 0.9.2 / 2026-11-19 | 38M | MIT OR Apache-2.0 | CSS colour parse and hex. Smaller than `color`. |
| `colorgrad` | 0.9.0 / 2026-09-28 | 1.7M | MIT OR Apache-2.0 | Gradients (RGB, Lab, Oklab), CSS gradient parsing, GIMP import. Not Blender interpolations. |

Recommendation: **replace the conversion layer now, keep the gamut and WGSL matrices**. Use `palette` for Oklab/Oklch/Lch/HSV conversions and check the results against `convert.rs` with tests (the WGSL codegen uses our matrices, so keep those constants and assert they match `palette`). Keep `display.rs`: no crate in the list does CSS Color 4 chroma-reduction gamut mapping, and Display P3 is ours. Use `color` only if CSS colour parsing is needed (e.g. importing a colour from text). Estimated removable: about 250 of 737 lines. Effort: small to medium, with the test-matching step being the main cost.

## Curves and beziers

`crates/graph-math/src/curve.rs` (904). Original: `packages/core/src/curve/curve.ts` (298).

Recommendation: **partial replacement now**. `kurbo` 0.13.1 (Apache-2.0 OR MIT, 72M downloads, Linebender) gives `CubicBez` evaluation, `subdivide`/split at a parameter, and root solving for x to t. Those cover `segment_point`, `segment_parameter` and `split_segment`, roughly 150 lines. Keep point kinds (`vector`, `free`, `aligned`, `mirrored`), smooth-handle rules and presets, which are the original's domain logic. Effort: small. Risk: kurbo's x-to-t solve must match the original's tolerance, so port the existing curve tests first.

## Ramps and gradients

`crates/graph-math/src/ramp.rs` (796), `crates/theme/src/ramp_editor.rs` (923), `crates/shader-graph/src/nodes/color_ramp.rs` (642). Original: `packages/core/src/ramp/ramp.ts` (373).

Recommendation: **keep in-house**. The interpolations are Blender's (Linear, Ease, Cardinal, B-Spline, Constant, Smooth with Steffen's monotone cubic) and each must be mirrored in WGSL codegen. No crate here provides them; `colorgrad` covers only Catmull-Rom, basis and linear modes. Use `colorgrad` only if GIMP or CSS gradient import is added later. Route colour-space conversions through the palette step above.

## Fuzzy search (add menu)

`crates/shader-graph/src/catalog.rs` search section (about lines 624 to 870, roughly 250 lines). Original: `packages/core/src/catalog/add-menu.ts` uses `@leeoniya/ufuzzy` with one-edit typo tolerance and unlimited intra-word insertions.

| Crate | Version / downloads | License | Notes |
|---|---|---|---|
| `nucleo-matcher` | 0.3.1 / 7.6M | MPL-2.0 (check) | Helix's fzf-style matcher, fast, returns match indices for highlighting. Different ranking from uFuzzy. |
| `frizbee` | 0.13.0 (2026-08-13) / 3.1M | MIT | Typo-tolerant matching. Closer to the typo requirement. |
| `sublime_fuzzy` | 0.7.0 / 3.7M | MIT | Simple subsequence matcher. Stale. |
| uFuzzy port | none found on crates.io | | |

Recommendation: **keep in-house for now**. The ranking semantics (label, aliases, skipped letters, typo tolerance, sections) are tested against the original, and no crate reproduces uFuzzy. Revisit by swapping the kernel for `frizbee` or `nucleo-matcher` only if the existing tests keep passing. Effort: medium, low priority.

## Flow canvas (node graph)

`crates/flow` (5,683 lines in total; `flow.rs` 2,194). Original: xyflow (`@xyflow/svelte`) in `packages/editor`.

Recommendation: **keep in-house**. The only gpui node-graph crates (listed above) target other gpui versions and are immature (under 500 downloads). The canvas is the core of the app, so owning it is reasonable.

## Dock and screen layout

`crates/screen` (2,279 lines: `view.rs` 1,003, `lib.rs` 692, `gestures.rs` 268, `tests.rs` 316), plus `crates/app/src/layout.rs` (232). Original: `packages/core/src/screen/screen.ts` (530), `ScreenLayout.svelte` (586).

Recommendation: **evaluate a spike, keep the model**. gpui-component's `dock` (DockArea, TabPanel, drag) and `resizable` cover the view and gesture parts. The screen format (areas, sizes, persisted layout) must stay compatible with the original, so keep `screen/src/lib.rs` (the layout tree and its serialisation) and test whether `dock` can host our areas. Effort: medium to large. Risk: dock drag-and-drop semantics may differ from ours.

## Text input, code view and WGSL highlighting

`crates/app/src/code_view.rs` (69). `crates/theme/src/input.rs`, `textarea.rs` (see theme table).

- gpui-component's highlighter has tree-sitter and syntect backends. Built-in languages: go, html, javascript, json, kotlin, lua, markdown, php, rust, typescript, zig. **No WGSL.**
- `tree-sitter-wgsl` 0.0.6 (2022-06-13, 10k downloads) is stale. Check that it parses current WGSL before using it.
- Option: a `.sublime-syntax` for WGSL with `syntect`, which gpui-component already supports.

Recommendation: **future feature** (code editing). Use `gpui-component`'s `Editor` with a WGSL grammar. Effort: small once a grammar is chosen.

## Undo history

`crates/shader-graph/src/graph/history.rs` (106). gpui-base (via gpui-component) exports `History<T>` and `UndoHistory`. The `undo` crate (0.52.0, 194k downloads, MIT OR Apache-2.0) offers a record/edit model.

Recommendation: **replace optional, low value**. Only worth doing if `History` in gpui-base matches our needs (back, forward, retain). Effort: small.

## Files, config and dialogs

| Ours | Lines | Candidate | Recommendation |
|---|---|---|---|
| `crates/app/src/project.rs` | 181 | none (domain) | Keep. |
| `crates/app/src/preferences.rs` | 137 | `directories` 6.0.0 / `etcetera` 0.11.0 for paths | Already `dirs` 7 in the app. Keep `dirs` (or `directories`). |
| `crates/app/src/settings.rs` | 760 | `confy` (2.0.0, 3M) | Keep. The format matches the original; `confy` would change it. |
| `crates/shader-graph/src/assets/bundle.rs` | 271 | `zip` 9.8.1 (already used) | Keep. Already the external crate. |
| File dialogs | in app | gpui-fast `prompt_for_paths` / `prompt_for_new_path` (built in) | Use gpui's built-in dialogs. Do not add `rfd` (47M downloads, but gpui covers it). |

Other dependencies match the original's: `sha2` and `uuid` are used here. `valibot` (schema validation) maps to serde derives. `fflate` maps to `zip` with deflate. `ropey` (2.0 beta) is pulled by gpui-component and can back text if needed.

## Proposed order of replacement tasks

1. **Add gpui-component.** Dependency with `default-features = false, features = ["gpui-fast"]`. Call `gpui_component::init` from `theme::init`. Map our shadcn tokens onto `ThemeColor`. Add the demo test.
2. **Leaf widgets.** Replace `Button`, `Kbd`, `Separator`, `Progress`, `Tooltip` with gpui-component equivalents. Keep the public API of `crates/theme` (`pub use`) so callers do not change.
3. **Toggles and popovers.** `Checkbox`, `RadioGroup`, `ToggleGroup`, `Popover`, `Menu`. Verify keyboard behaviour with TestAppContext.
4. **Text inputs.** `Input` + `NumberInput` (keep the scrub layer), then `Textarea`. Keep behaviour tests in `crates/theme` and `crates/flow/tests/pointer.rs`.
5. **Select and dialog.** `Select` (keep the reopen rule), `Dialog`/`Sheet`.
6. **Curve maths.** Use `kurbo` for segment evaluate, split and x-to-t solve in `graph-math/src/curve.rs`. Keep point kinds and presets.
7. **Colour conversion.** Replace conversions in `graph-math/src/color/convert.rs` with `palette`, keeping WGSL matrices and display gamut. Tests must match the existing values.
8. **Icon decision.** Keep Phosphor SVGs or move to Lucide (`gpui-kit-assets`). Decide with the user.
9. **Dock spike.** Prototype `gpui-component` dock with our screen tree. Go or no-go.
10. **Future features, each its own task:**
    - WGSL code view via `gpui-component` Editor and a WGSL grammar.
    - Auto-layout via `elk-layout`, behind a trait.
    - Video export: `VideoEncoder` trait, AVFoundation first, `ffmpeg-next` behind a feature.
    - GIF export via `gifski`.
    - Video import: AVFoundation (macOS) or `ffmpeg-next`.
    - Fuzzy search kernel swap (optional, last).

## Risks

- gpui-component pulls `gpui-pre` (second gpui) even with the `gpui-fast` feature. Watch compile time and binary size.
- gpui-component's widgets have their own event models. Parity tests are needed for every replacement.
- `elk-layout` has low download counts and a dual licence (EPL-2.0 or GPL-3.0-or-later). Verify licence compatibility with AGPL-3.0-or-later.
- `gifski` is AGPL-3.0-or-later: compatible with this project.
- H.264 via `openh264` or FFmpeg: codec licence and patent terms for distributed binaries.
- AVFoundation is macOS-only: keep a cross-platform path before committing.
