//! Bringing files from disk into a project as its assets.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Image, ImageFormat, ImageSource, Pixels, Point,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, PromptLevel, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyleRefinement, Subscription, WeakEntity, Window,
    StyledImage as _, ObjectFit, div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_component::Sizable as _;
use runtime::{MediaStatus, SystemVideoProbe};
use shader_graph::assets::{Asset, AssetKind, ImportFile, describe_asset};
use shader_graph::graph::GraphModel;
use theme::{ActiveTheme as _, Icon, IconName, popup};

use crate::editor::{Editor, EditorEvent};
use crate::playback::Playback;

/// A file read from disk, ready to import.
pub struct PickedFile {
    pub name: String,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
}

impl PickedFile {
    /// The file's media type comes from its extension, as the platform's file picker gives it.
    pub fn read(path: &Path) -> Result<Self, String> {
        let name = crate::project::file_name(path);
        let bytes = fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let mime = mime_guess::from_path(path).first_or_octet_stream().essence_str().to_owned();
        Ok(Self { name, mime, bytes: Arc::new(bytes) })
    }
}

/// The result of an import: each file's asset with its bytes, and what couldn't be imported.
#[derive(Default)]
pub struct Imports {
    pub files: Vec<(String, Arc<Vec<u8>>)>,
    pub problems: Vec<String>,
}

/// Imports the files as one undo step. A file already in the project gives its existing asset.
pub fn import(model: &mut GraphModel, picked: &[PickedFile]) -> Imports {
    let mut imports = Imports::default();
    model.batched(|model| {
        for file in picked {
            let input = ImportFile { name: &file.name, mime: &file.mime, bytes: &file.bytes };
            match model.import_file(input, &SystemVideoProbe) {
                Ok(imported) => imports.files.push((imported.asset.id().to_owned(), file.bytes.clone())),
                Err(error) => imports.problems.push(format!("{}: {error}", file.name)),
            }
        }
    });
    imports
}

/// Asks for media files and imports the ones picked: File › Import media and the panel's Import button.
pub fn pick_and_import<T: 'static>(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
    let editor = editor.clone();
    let playback = playback.clone();
    cx.spawn_in(window, async move |_, cx| {
        let picked = match paths.await {
            Ok(Ok(Some(paths))) => paths,
            _ => return,
        };
        cx.update(|window, cx| import_paths(&editor, &playback, &picked, window, cx)).ok();
    })
    .detach();
}

/// Reads the paths; the ones that can't be read are returned as problems.
fn read_paths(paths: &[PathBuf]) -> (Vec<PickedFile>, Vec<String>) {
    let mut picked = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match PickedFile::read(path) {
            Ok(file) => picked.push(file),
            Err(problem) => problems.push(problem),
        }
    }
    (picked, problems)
}

/// Imports the files as one undo step, with their bytes kept by the playback. When `node` is
/// given, it is set to read the first file imported, in the same undo step.
fn import_into(
    editor: &mut Editor,
    playback: &Entity<Playback>,
    picked: &[PickedFile],
    node: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Imports {
    let mut imports = Imports::default();
    editor.edit_model(cx, |model| {
        model.batched(|model| {
            imports = import(model, picked);
            if let (Some(node), Some((id, _))) = (node, imports.files.first()) {
                model.set_node_asset(node, Some(id));
            }
        })
    });
    if let (Some(node), Some((id, _))) = (node, imports.files.first()) {
        let reads_it = editor.model().node(node).and_then(|n| n.data.asset.as_deref()) == Some(id.as_str());
        if !reads_it {
            let name = editor.model().asset(id).map_or_else(String::new, |a| a.name().to_owned());
            imports.problems.push(format!("{name}: this node can't read it, being of another kind."));
        }
    }
    let files = imports.files.clone();
    playback.update(cx, |p, cx| {
        let mut all = p.asset_files().clone();
        for (id, bytes) in files {
            all.entry(id).or_insert(bytes);
        }
        p.set_asset_files(all, window, cx);
    });
    imports
}

/// Reads the paths and imports what they hold as one undo step. Files that can't be used are reported together.
pub fn import_paths(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    paths: &[PathBuf],
    window: &mut Window,
    cx: &mut App,
) {
    let (picked, mut problems) = read_paths(paths);
    let imports = editor.update(cx, |e, cx| import_into(e, playback, &picked, None, window, cx));
    problems.extend(imports.problems);
    report_problems(window, &problems, cx);
}

/// Asks for one media file, imports it and makes `node`, an Image or Video node, read it: its picker's Import entry.
pub fn pick_and_import_for_node<T: 'static>(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    node: &str,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
    let (editor, playback, node) = (editor.clone(), playback.clone(), node.to_owned());
    cx.spawn_in(window, async move |_, cx| {
        let picked = match paths.await {
            Ok(Ok(Some(paths))) => paths,
            _ => return,
        };
        cx.update(|window, cx| import_for_node(&editor, &playback, &node, &picked, window, cx)).ok();
    })
    .detach();
}

/// Imports the file at `paths` and makes `node` read it, as one undo step.
pub fn import_for_node(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    node: &str,
    paths: &[PathBuf],
    window: &mut Window,
    cx: &mut App,
) {
    let (picked, mut problems) = read_paths(paths);
    let imports = editor.update(cx, |e, cx| import_into(e, playback, &picked, Some(node), window, cx));
    problems.extend(imports.problems);
    report_problems(window, &problems, cx);
}

/// Imports files dropped on the graph and makes nodes of them where they land.
pub fn drop_files_on_graph(
    editor: &mut Editor,
    playback: &Entity<Playback>,
    paths: &[PathBuf],
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let (picked, mut problems) = read_paths(paths);
    let imports = import_into(editor, playback, &picked, None, window, cx);
    problems.extend(imports.problems);
    let ids: Vec<String> = imports.files.iter().map(|(id, _)| id.clone()).collect();
    if !ids.is_empty() {
        editor.drop_assets(&ids, at, cx);
    }
    report_problems(window, &problems, cx);
}

/// Shows the problems in one dialog; nothing is shown when there are none.
pub fn report_problems(window: &mut Window, problems: &[String], cx: &mut App) {
    if problems.is_empty() {
        return;
    }
    log::warn!("left out of the project: {}", problems.join("; "));
    let mut detail = "What couldn't be read was left out.".to_owned();
    for problem in problems {
        detail.push_str(&format!("\n• {problem}"));
    }
    drop(window.prompt(PromptLevel::Warning, "Some files couldn't be used", Some(&detail), &["OK"], cx));
}

/// The media type of an image file's bytes, for showing them.
pub(crate) fn image_format(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        "image/bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// What an Assets tile carries while it is dragged onto a graph: the asset a node is added for.
#[derive(Clone)]
pub struct AssetDrag {
    pub id: String,
    pub name: String,
}

/// The chip that follows the pointer while an asset is dragged.
struct AssetDragPreview {
    name: SharedString,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("asset drag", || {
            let theme = cx.theme().clone();
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.colors.popover)
                .text_size(px(12.0))
                .child(self.name.clone())
        })
    }
}

/// What a tile shows of one asset: its metadata, its file if the project has it, and how many nodes read it.
struct Tile {
    panel: WeakEntity<AssetsPanel>,
    asset: Asset,
    file: Option<Arc<Vec<u8>>>,
    usage: usize,
    status: Option<MediaStatus>,
    thumbnail: Option<Arc<Image>>,
    rename: Option<(Entity<InputState>, Subscription)>,
}

impl Tile {
    fn set(
        &mut self,
        asset: &Asset,
        file: Option<&Arc<Vec<u8>>>,
        usage: usize,
        status: Option<MediaStatus>,
        cx: &mut Context<Self>,
    ) {
        let same_file = match (&self.file, file) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if self.asset == *asset && same_file && self.usage == usage && self.status == status {
            return;
        }
        if !same_file || self.asset.file_info().mime != asset.file_info().mime {
            self.thumbnail = file.and_then(|bytes| {
                let format = image_format(&asset.file_info().mime)?;
                Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
            });
        }
        self.asset = asset.clone();
        self.file = file.cloned();
        self.usage = usage;
        self.status = status;
        cx.notify();
    }

    fn details(&self) -> String {
        let info = self.asset.file_info();
        match self.asset.kind() {
            AssetKind::Image => format!("{} × {}", info.width, info.height),
            AssetKind::Video => format!("{} × {} · video", info.width, info.height),
        }
    }

    fn usage_text(&self) -> String {
        match self.usage {
            0 => "Unused".to_owned(),
            1 => "Used by 1 node".to_owned(),
            n => format!("Used by {n} nodes"),
        }
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.asset.name().to_owned();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(name, window, cx);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let id = self.asset.id().to_owned();
        let subscription = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _window, cx| {
            guard::guard("rename asset", || match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let text = input.read(cx).value().to_string();
                    this.rename = None;
                    if let Some(panel) = this.panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.rename(&id, &text, cx));
                    }
                    cx.notify();
                }
                _ => {}
            });
        });
        self.rename = Some((input, subscription));
        cx.notify();
    }
}

impl Render for Tile {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("asset tile", || {
            let theme = cx.theme().clone();
            let id = self.asset.id().to_owned();
            let panel = self.panel.clone();
            let missing = self.file.is_none();
            let failed = matches!(self.status, Some(MediaStatus::Failed(_)));
            let status_text = match &self.status {
                Some(MediaStatus::Loading) => Some("Loading…".to_owned()),
                Some(MediaStatus::Failed(message)) => Some(message.clone()),
                _ => None,
            };
            let preview: AnyElement = match (&self.thumbnail, self.asset.kind(), missing || failed) {
                (_, _, true) => Icon::new(IconName::Warning).size(px(20.0)).into_any_element(),
                (Some(image), AssetKind::Image, false) => img(ImageSource::Image(image.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                _ => Icon::new(IconName::FilmStrip).size(px(20.0)).into_any_element(),
            };
            let name: AnyElement = match &self.rename {
                Some((input, _)) => Input::new(input).small().appearance(false).into_any_element(),
                None => div()
                    .truncate()
                    .child(self.asset.name().to_owned())
                    .into_any_element(),
            };
            let tile_weak = cx.entity().downgrade();
            let click_tile = tile_weak.clone();
            let tile_id = format!("asset-{id}");
            let drag = AssetDrag { id: id.clone(), name: self.asset.name().to_owned() };
            div()
                .id(SharedString::from(tile_id))
                .debug_selector(|| "asset-tile".into())
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .min_w_0()
                .rounded_md()
                .hover(|s| s.bg(theme.colors.accent.opacity(0.5)))
                .on_drag(drag, |drag, _, _, cx| {
                    cx.new(|_| AssetDragPreview { name: SharedString::from(drag.name.clone()) })
                })
                .on_click(move |event, window, cx| {
                    if event.click_count() != 2 {
                        return;
                    }
                    if let Some(tile) = click_tile.upgrade() {
                        tile.update(cx, |tile, cx| tile.start_rename(window, cx));
                    }
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .aspect_ratio(16.0 / 9.0)
                        .overflow_hidden()
                        .rounded_sm()
                        .bg(theme.colors.muted)
                        .text_color(theme.colors.muted_foreground)
                        .child(preview),
                )
                .child(div().text_size(px(12.0)).min_w_0().child(name))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.colors.muted_foreground)
                        .truncate()
                        .child(match (missing, status_text) {
                            (true, _) => "File missing".to_owned(),
                            (false, Some(text)) => text,
                            (false, None) => self.details(),
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.colors.muted_foreground)
                        .truncate()
                        .child(self.usage_text()),
                )
                .context_menu(move |menu, _, _| {
                    let replace_panel = panel.clone();
                    let delete_panel = panel.clone();
                    let (replace_id, delete_id) = (id.clone(), id.clone());
                    let rename_tile = tile_weak.clone();
                    menu.min_w(px(176.0))
                        .item(PopupMenuItem::new("Rename").on_click(popup::on_select(move |window, cx| {
                            if let Some(tile) = rename_tile.upgrade() {
                                tile.update(cx, |tile, cx| tile.start_rename(window, cx));
                            }
                        })))
                        .item(PopupMenuItem::new("Replace file…").on_click(popup::on_select(move |window, cx| {
                            if let Some(panel) = replace_panel.upgrade() {
                                panel.update(cx, |panel, cx| panel.replace(&replace_id, window, cx));
                            }
                        })))
                        .separator()
                        .item(PopupMenuItem::new("Delete").on_click(popup::on_select(move |window, cx| {
                            if let Some(panel) = delete_panel.upgrade() {
                                panel.update(cx, |panel, cx| panel.delete(&delete_id, window, cx));
                            }
                        })))
                })
        })
    }
}

/// The narrowest a tile gets.
const TILE_WIDTH: f32 = 120.0;

/// The Assets area: a grid of the project's assets, each a cached tile.
pub struct AssetsPanel {
    editor: Entity<Editor>,
    playback: Entity<Playback>,
    tiles: Vec<Entity<Tile>>,
    /// The files the tiles were last built from, by asset id and file identity.
    files_seen: MediaSignature,
    _subscriptions: Vec<Subscription>,
}

/// Which files the playback holds and how the renderer reads each, by asset id. Files are compared by identity.
type MediaSignature = Vec<(String, usize, Option<MediaStatus>)>;

fn media_signature(playback: &Playback) -> MediaSignature {
    playback
        .asset_files()
        .iter()
        .map(|(id, bytes)| (id.clone(), Arc::as_ptr(bytes) as usize, playback.media_status(id)))
        .collect()
}

impl AssetsPanel {
    pub fn new(editor: Entity<Editor>, playback: Entity<Playback>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, _, cx| {
                if let EditorEvent::Edited = event {
                    this.refresh(cx);
                }
            }),
            cx.observe(&playback, |this, playback, cx| {
                let signature = media_signature(playback.read(cx));
                if signature != this.files_seen {
                    this.refresh(cx);
                }
            }),
        ];
        let mut panel =
            Self { editor, playback, tiles: Vec::new(), files_seen: Vec::new(), _subscriptions: subscriptions };
        panel.refresh(cx);
        panel
    }

    /// Brings the tiles to the project's assets. A tile is only updated when what it shows changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let (assets, usages) = {
            let model = self.editor.read(cx).model();
            (model.assets().to_vec(), model.asset_usages())
        };
        let files = self.playback.read(cx).asset_files().clone();
        self.files_seen = media_signature(self.playback.read(cx));
        let before: Vec<_> = self.tiles.iter().map(|t| t.entity_id()).collect();
        let mut tiles = Vec::with_capacity(assets.len());
        for asset in &assets {
            let usage = usages.get(asset.id()).copied().unwrap_or(0);
            let file = files.get(asset.id());
            let status = self.playback.read(cx).media_status(asset.id());
            let existing = self.tiles.iter().find(|tile| tile.read(cx).asset.id() == asset.id()).cloned();
            let tile = existing.unwrap_or_else(|| {
                let panel = me.clone();
                let asset = asset.clone();
                cx.new(|_| Tile { panel, asset, file: None, usage: 0, status: None, thumbnail: None, rename: None })
            });
            tile.update(cx, |tile, cx| tile.set(asset, file, usage, status, cx));
            tiles.push(tile);
        }
        if tiles.iter().map(|t| t.entity_id()).ne(before) {
            cx.notify();
        }
        self.tiles = tiles;
    }

    fn rename(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let name = name.to_owned();
        let id = id.to_owned();
        self.editor.update(cx, |e, cx| e.edit_model(cx, |m| m.rename_asset(&id, &name)));
    }

    fn delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let usage = self.editor.read(cx).model().asset_usages().get(id).copied().unwrap_or(0);
        let name = self.editor.read(cx).model().asset(id).map(|a| a.name().to_owned()).unwrap_or_default();
        if usage == 0 {
            self.remove(id, cx);
            return;
        }
        let used = if usage == 1 { "Used by 1 node".to_owned() } else { format!("Used by {usage} nodes") };
        let detail = format!("{used}, which will read a transparent image instead.\nUndo brings it back.");
        let answer = window.prompt(PromptLevel::Warning, &format!("Delete {name}?"), Some(&detail), &["Delete", "Cancel"], cx);
        let id = id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.remove(&id, cx)).ok();
            }
        })
        .detach();
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.editor.update(cx, |e, cx| e.edit_model(cx, |m| m.remove_asset(&id)));
    }

    /// Gives the asset another file of its kind, keeping its id, from a file the user picks.
    fn replace(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let id = id.to_owned();
        let editor = self.editor.clone();
        let playback = self.playback.clone();
        cx.spawn_in(window, async move |_, cx| {
            let picked = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else { return };
            cx.update(|window, cx| replace_file(&editor, &playback, &id, &path, window, cx)).ok();
        })
        .detach();
    }
}

/// Replaces an asset's file with the one at `path`, if that is an asset of the same kind.
fn replace_file(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    id: &str,
    path: &Path,
    window: &mut Window,
    cx: &mut App,
) {
    let file = match PickedFile::read(path) {
        Ok(file) => file,
        Err(problem) => return report_problems(window, &[problem], cx),
    };
    let kind = editor.read(cx).model().asset(id).map(Asset::kind);
    let replacement = describe_asset(id, ImportFile { name: &file.name, mime: &file.mime, bytes: &file.bytes }, &SystemVideoProbe);
    match (kind, replacement) {
        (Some(kind), Ok(asset)) if asset.kind() == kind => {
            editor.update(cx, |e, cx| e.edit_model(cx, |m| m.replace_asset(asset)));
            let bytes = file.bytes.clone();
            let id = id.to_owned();
            playback.update(cx, |p, cx| {
                let mut all = p.asset_files().clone();
                all.insert(id, bytes);
                p.set_asset_files(all, window, cx);
            });
        }
        _ => report_problems(
            window,
            &[format!("{}: it can't replace this asset, being unreadable or of another kind.", file.name)],
            cx,
        ),
    }
}

impl Render for AssetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("assets panel", || {
            let theme = cx.theme().clone();
            let panel = cx.entity().downgrade();
            let import = Button::new("import-media")
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .label("Import")
                .debug_selector(|| "assets-import".into())
                .on_click(move |_, window, cx| {
                    if let Some(panel) = panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.import_media(window, cx));
                    }
                });
            let body = if self.tiles.is_empty() {
                div()
                    .flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_center()
                    .text_size(px(12.0))
                    .text_color(theme.colors.muted_foreground)
                    .child("Drop images and videos here, or import them.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.tiles.iter().map(|tile| {
                        div()
                            .flex_grow_1()
                            .flex_basis(px(TILE_WIDTH))
                            .min_w(px(TILE_WIDTH))
                            .child(tile.clone().cached(StyleRefinement::default()))
                    }))
                    .into_any_element()
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme.colors.background)
                .child(div().flex().justify_end().p_1().child(import))
                .child(div().id("assets-body").flex_1().min_h_0().overflow_y_scroll().p_2().child(body))
        })
    }
}

impl AssetsPanel {
    fn import_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        pick_and_import(&self.editor, &self.playback, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_graph::graph::Point;
    use shader_graph::nodes::IMAGE;
    use std::fs;

    fn png(width: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, 1, image::Rgba([9, 9, 9, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn picked(name: &str, bytes: Vec<u8>) -> PickedFile {
        PickedFile { name: name.into(), mime: "image/png".into(), bytes: Arc::new(bytes) }
    }

    #[test]
    fn importing_adds_the_assets_and_is_one_undo_step() {
        let mut model = GraphModel::default();
        let imports = import(&mut model, &[picked("a.png", png(1)), picked("b.png", png(2))]);
        assert_eq!(model.assets().len(), 2);
        assert_eq!(imports.files.len(), 2);
        assert!(imports.problems.is_empty());
        assert!(model.has_unsaved_changes());

        assert!(model.undo());
        assert!(model.assets().is_empty(), "one undo takes back the whole import");
        assert!(!model.has_unsaved_changes());
    }

    #[test]
    fn importing_the_same_file_twice_reuses_its_asset() {
        let mut model = GraphModel::default();
        let first = import(&mut model, &[picked("a.png", png(1))]);
        let second = import(&mut model, &[picked("copy.png", png(1))]);
        assert_eq!(model.assets().len(), 1);
        assert_eq!(first.files[0].0, second.files[0].0);
    }

    #[test]
    fn a_file_that_cant_be_imported_is_reported_and_adds_nothing() {
        let mut model = GraphModel::default();
        let notes = PickedFile { name: "notes.txt".into(), mime: "text/plain".into(), bytes: Arc::new(b"hi".to_vec()) };
        let imports = import(&mut model, &[notes]);
        assert!(imports.files.is_empty());
        assert_eq!(imports.problems.len(), 1);
        assert!(imports.problems[0].starts_with("notes.txt: "), "{:?}", imports.problems);
        assert!(model.assets().is_empty());
        assert!(!model.has_unsaved_changes());
    }

    #[test]
    fn a_renamed_asset_keeps_its_id_and_a_deleted_one_is_gone() {
        let mut model = GraphModel::default();
        let imports = import(&mut model, &[picked("a.png", png(1))]);
        let id = imports.files[0].0.clone();
        model.rename_asset(&id, "  Sky  ");
        assert_eq!(model.asset(&id).map(|a| a.name().to_owned()), Some("Sky".to_owned()));
        model.rename_asset(&id, "   ");
        assert_eq!(model.asset(&id).map(|a| a.name().to_owned()), Some("Sky".to_owned()));
        model.remove_asset(&id);
        assert!(model.asset(&id).is_none());
    }

    #[test]
    fn a_picked_file_gets_its_media_type_from_its_extension() {
        let dir = std::env::temp_dir().join(format!("wgnodes-assets-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("Photo.png");
        fs::write(&path, png(1)).expect("writes");
        let file = PickedFile::read(&path).expect("reads");
        assert_eq!(file.name, "Photo.png");
        assert_eq!(file.mime, "image/png");
        assert!(PickedFile::read(&dir.join("missing.png")).is_err());
        fs::remove_dir_all(dir).ok();
    }

    fn scratch_png(dir: &Path, name: &str, width: u32) -> PathBuf {
        fs::create_dir_all(dir).expect("scratch dir");
        let path = dir.join(name);
        fs::write(&path, png(width)).expect("writes");
        path
    }

    /// An editor, playback and the Assets panel over them, in a window.
    fn panel(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Editor>, Entity<Playback>, Entity<AssetsPanel>, gpui::VisualTestContext) {
        cx.update(theme::init);
        let (editor, vcx) = cx.add_window_view(Editor::new);
        let playback = vcx.new(|_| Playback::new());
        let panel = vcx.update(|window, cx| {
            cx.new(|cx| AssetsPanel::new(editor.clone(), playback.clone(), window, cx))
        });
        vcx.run_until_parked();
        (editor, playback, panel, vcx.clone())
    }

    fn asset_count(editor: &Entity<Editor>, vcx: &mut gpui::VisualTestContext) -> usize {
        editor.read_with(vcx, |e, _| e.model().assets().len())
    }

    #[gpui::test]
    fn import_for_node_sets_the_node_and_is_one_undo_step(cx: &mut gpui::TestAppContext) {
        let (editor, playback, _panel, mut vcx) = panel(cx);
        let node = editor.update(&mut vcx, |e, cx| {
            let mut id = String::new();
            e.edit_model(cx, |m| id = m.add_node(IMAGE, Point::new(0.0, 0.0)).expect("adds"));
            id
        });
        let dir = std::env::temp_dir().join(format!("wgnodes-import-for-node-{}", std::process::id()));
        let path = scratch_png(&dir, "Photo.png", 3);

        vcx.update(|window, cx| import_for_node(&editor, &playback, &node, std::slice::from_ref(&path), window, cx));
        vcx.run_until_parked();
        let read = |vcx: &mut gpui::VisualTestContext| {
            editor.read_with(vcx, |e, _| e.model().node(&node).and_then(|n| n.data.asset.clone()))
        };
        assert!(read(&mut vcx).is_some(), "the node reads the imported file");
        assert_eq!(asset_count(&editor, &mut vcx), 1);

        editor.update(&mut vcx, |e, cx| e.undo(cx));
        vcx.run_until_parked();
        assert_eq!(read(&mut vcx), None, "one undo takes back the node's asset");
        assert_eq!(asset_count(&editor, &mut vcx), 0, "and the import with it");
        fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn importing_shows_the_asset_and_undo_takes_it_back(cx: &mut gpui::TestAppContext) {
        let (editor, playback, panel, mut vcx) = panel(cx);
        let dir = std::env::temp_dir().join(format!("wgnodes-import-{}", std::process::id()));
        let path = scratch_png(&dir, "Photo.png", 3);

        vcx.update(|window, cx| import_paths(&editor, &playback, std::slice::from_ref(&path), window, cx));
        vcx.run_until_parked();
        assert_eq!(asset_count(&editor, &mut vcx), 1);
        assert_eq!(panel.read_with(&vcx, |p, _| p.tiles.len()), 1, "the grid shows the new asset");
        assert_eq!(playback.read_with(&vcx, |p, _| p.asset_files().len()), 1, "its file is kept");
        assert!(editor.read_with(&vcx, |e, _| e.has_unsaved_changes()));

        editor.update(&mut vcx, |e, cx| e.undo(cx));
        vcx.run_until_parked();
        assert_eq!(asset_count(&editor, &mut vcx), 0);
        assert_eq!(panel.read_with(&vcx, |p, _| p.tiles.len()), 0);
        fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn importing_the_same_file_again_reuses_its_asset(cx: &mut gpui::TestAppContext) {
        let (editor, playback, _panel, mut vcx) = panel(cx);
        let dir = std::env::temp_dir().join(format!("wgnodes-reuse-{}", std::process::id()));
        let path = scratch_png(&dir, "Photo.png", 3);
        let copy = scratch_png(&dir.join("again"), "Copy.png", 3);

        vcx.update(|window, cx| import_paths(&editor, &playback, std::slice::from_ref(&path), window, cx));
        vcx.update(|window, cx| import_paths(&editor, &playback, std::slice::from_ref(&copy), window, cx));
        vcx.run_until_parked();
        assert_eq!(asset_count(&editor, &mut vcx), 1);
        fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn renaming_and_deleting_an_unused_asset(cx: &mut gpui::TestAppContext) {
        let (editor, playback, panel, mut vcx) = panel(cx);
        let dir = std::env::temp_dir().join(format!("wgnodes-rename-{}", std::process::id()));
        let path = scratch_png(&dir, "Photo.png", 3);
        vcx.update(|window, cx| import_paths(&editor, &playback, std::slice::from_ref(&path), window, cx));
        vcx.run_until_parked();
        let id = editor.read_with(&vcx, |e, _| e.model().assets()[0].id().to_owned());

        panel.update(&mut vcx, |p, cx| p.rename(&id, "Sky", cx));
        vcx.run_until_parked();
        assert_eq!(editor.read_with(&vcx, |e, _| e.model().asset(&id).map(|a| a.name().to_owned())), Some("Sky".into()));

        panel.update(&mut vcx, |p, cx| p.remove(&id, cx));
        vcx.run_until_parked();
        assert_eq!(asset_count(&editor, &mut vcx), 0);
        assert_eq!(panel.read_with(&vcx, |p, _| p.tiles.len()), 0);
        fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn replacing_keeps_the_asset_id(cx: &mut gpui::TestAppContext) {
        let (editor, playback, _panel, mut vcx) = panel(cx);
        let dir = std::env::temp_dir().join(format!("wgnodes-replace-{}", std::process::id()));
        let path = scratch_png(&dir, "Photo.png", 3);
        let other = scratch_png(&dir.join("other"), "Other.png", 5);
        vcx.update(|window, cx| import_paths(&editor, &playback, std::slice::from_ref(&path), window, cx));
        vcx.run_until_parked();
        let (id, hash) = editor.read_with(&vcx, |e, _| {
            let asset = e.model().assets()[0].clone();
            (asset.id().to_owned(), asset.hash().to_owned())
        });

        vcx.update(|window, cx| replace_file(&editor, &playback, &id, &other, window, cx));
        vcx.run_until_parked();
        let (count, new_hash, size) = editor.read_with(&vcx, |e, _| {
            let model = e.model();
            let asset = model.asset(&id).expect("the asset keeps its id");
            (model.assets().len(), asset.hash().to_owned(), asset.file_info().width)
        });
        assert_eq!(count, 1);
        assert_ne!(new_hash, hash);
        assert_eq!(size, 5);
        fs::remove_dir_all(dir).ok();
    }
}

