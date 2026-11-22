# Feature parity plan

What the port still lacks compared with the original (packages/core, packages/runtime, packages/editor, apps/web), in the order it is being brought in. Order follows dependencies first, then how much each piece unblocks.

## 1. Model foundations
1. **Auto layout**: `layout/layout.ts` (computeLayout, estimateNodeGeometry, layoutDocument) through the `elk-layout` crate (see crate-plan.md); then Edit › Layout selected / graph / document, and the add menu's Layout › Named reroutes category.
2. **Load hardening**: `graph/limits.ts` constants and the `hydrate.ts` normalise pass enforced on load; `validate.test.ts`, `import-groups.test.ts`, `state-size.test.ts` ports.
3. **Groups**: Clone & Edit for locked groups (deep clone), Prune unused groups (query and confirm dialog), outdated-node warning.

## 2. Assets
1. **Media library**: image texture cache and upload (`runtime/media.ts`), Import media with hash dedupe (`graph.importFiles`, `describeAsset`).
2. **Assets panel**: `Assets.svelte` area type (grid, rename, replace, delete), drag an asset onto the graph, drop files on the window.
3. **Asset widget** on nodes (picker, speed, after-end, format) and missing-file warnings.
4. **Group bundles**: Export selected group, Import groups (with bundled assets).
5. **Video decode** (FFmpeg, statically built through `ffmpeg-next`, one backend on every platform).

## 3. Output
1. **Video export**: `VideoExportDialog.svelte` and `exports.ts` (format, quality, range, fps, progress).

## 4. Editor completeness
1. **Preview node**: ⇧⌘-click and the node menu's Preview item.
2. **Menus**: the original's full File / Edit / Display menus, each item wired to its action.
3. **Shortcuts and guards**: preview zoom keys (+ − 0 1), ⌘M light/dark, unsaved-changes prompt on close and quit, one aggregated "Some files couldn't be used" dialog.
4. **Developer tools**: toggle, dev nodes (`test-invalid-wgsl`, `test-runtime-error`), add-menu developer entries, code logging.

## 5. Tests
Port the original tests that still have no counterpart (layout, simplex, vector, controls, invalidation, unavailable-links, views, stdlib groups), and deepen history, values and zones coverage.

Out of scope: the web landing page, the stdlib layout dev server, archived library versions (disabled in the original).
