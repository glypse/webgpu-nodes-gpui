use guard::SafeListener as _;
use gpui::prelude::FluentBuilder as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, ExternalPaths, Focusable as _, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, PathPromptOptions, PromptLevel, Render, Styled as _, StyleRefinement, Window,
    actions, div,
};
use runtime::{Size, Timing};
use screen::{AreaContent, AreaId, Axis, HEADER_HEIGHT, MinSize, Part, Rect, Screen, ScreenView};
use shader_graph::assets::BundleKind;
use shader_graph::graph::{GraphProject, ProjectSettings};
use theme::{ActiveTheme as _, IconName};

use crate::assets::{self, AssetsPanel};
use crate::code_view::{self, CodeView};
use crate::editor::{Editor, EditorEvent};
use crate::group_bundles;
use crate::graph::breadcrumbs::Breadcrumbs;
use crate::layout::{self, GraphView};
use crate::output;
use crate::playback::{Playback, PlaybackEvent};
use crate::preferences::{self, Preferences};
use crate::project::{self, AssetFiles, Loaded};
use crate::properties::{PropertiesPanel, Section};
use crate::preview::{self, PreviewArea, PreviewToolbar};
use crate::timeline::TimelineBar;
use crate::video_export;
use crate::windows::{self, CloseWindow, NewOutputWindow};

actions!(
    shell,
    [
        TogglePlayback,
        PreviousFrame,
        NextFrame,
        GoToStart,
        NewProject,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        OpenProjectSettings,
        OpenPreferences,
        ExportFrame,
        ToggleMode,
        ZoomPreviewIn,
        ZoomPreviewOut,
        FitPreview,
        ActualSizePreview,
        MenuZoomPreviewIn,
        MenuZoomPreviewOut,
        MenuFitPreview,
        MenuActualSizePreview,
        ImportMedia,
        ImportGroups,
        ExportVideo,
    ]
);

/// Playback keys act anywhere but while typing in a field.
const CONTEXT: &str = "Shell && !Input";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, Some(CONTEXT)),
        KeyBinding::new(",", PreviousFrame, Some(CONTEXT)),
        KeyBinding::new(".", NextFrame, Some(CONTEXT)),
        KeyBinding::new("home", GoToStart, Some(CONTEXT)),
        KeyBinding::new("cmd-n", NewProject, Some("Shell")),
        KeyBinding::new("cmd-o", OpenProject, Some("Shell")),
        KeyBinding::new("cmd-s", SaveProject, Some("Shell")),
        KeyBinding::new("cmd-shift-s", SaveProjectAs, Some("Shell")),
        KeyBinding::new("cmd-,", OpenPreferences, Some("Shell")),
        KeyBinding::new("cmd-m", ToggleMode, Some("Shell")),
        // Both keys of "+": typed with shift on US layouts, where only the typed character matches.
        KeyBinding::new("+", ZoomPreviewIn, Some(CONTEXT)),
        KeyBinding::new("=", ZoomPreviewIn, Some(CONTEXT)),
        KeyBinding::new("-", ZoomPreviewOut, Some(CONTEXT)),
        KeyBinding::new("0", FitPreview, Some(CONTEXT)),
        KeyBinding::new("1", ActualSizePreview, Some(CONTEXT)),
    ]);
    cx.observe_global::<Preferences>(crate::refresh_menus).detach();
}

/// The kinds of panel an area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Graph,
    Preview,
    Timeline,
    Code,
    Assets,
    Properties,
}

impl screen::Editor for Panel {
    fn all() -> &'static [Self] {
        &[Self::Graph, Self::Preview, Self::Timeline, Self::Code, Self::Assets, Self::Properties]
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Graph => "Node graph",
            Self::Preview => "Preview",
            Self::Timeline => "Timeline",
            Self::Code => "WGSL",
            Self::Assets => "Assets",
            Self::Properties => "Properties",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Graph => IconName::TreeStructure,
            Self::Preview => IconName::MonitorPlay,
            Self::Timeline => IconName::FilmStrip,
            Self::Code => IconName::Code,
            Self::Assets => IconName::Palette,
            Self::Properties => IconName::Faders,
        }
    }
}

fn default_screen() -> Screen<Panel> {
    Screen::new([
        (Rect::new(0.0, 0.0, 0.6, 0.9), Panel::Graph),
        (Rect::new(0.0, 0.9, 0.6, 1.0), Panel::Timeline),
        (Rect::new(0.6, 0.0, 1.0, 0.5), Panel::Preview),
        (Rect::new(0.6, 0.5, 1.0, 0.8), Panel::Properties),
        (Rect::new(0.6, 0.8, 1.0, 1.0), Panel::Code),
    ])
}

/// The narrowest a split-off Properties area may be, in points.
const PROPERTIES_MIN_WIDTH: f64 = 320.0;

/// Splits the largest area of `screen` in two, the new half showing Properties. `None` when the window has no room for it.
fn split_for_properties(screen: &Screen<Panel>, window: &Window) -> Option<Screen<Panel>> {
    let viewport = window.viewport_size();
    let (width, height) = (f64::from(viewport.width).max(1.0), f64::from(viewport.height).max(1.0));
    let largest = screen
        .areas()
        .iter()
        .max_by(|a, b| (a.rect.width() * a.rect.height()).total_cmp(&(b.rect.width() * b.rect.height())))?;
    let rect = largest.rect;
    let (axis, at) = if rect.width() * width >= rect.height() * height {
        (Axis::X, (rect.left + rect.right) / 2.0)
    } else {
        (Axis::Y, (rect.top + rect.bottom) / 2.0)
    };
    let min = MinSize { width: PROPERTIES_MIN_WIDTH / width, height: f64::from(HEADER_HEIGHT) / height };
    let (mut next, id) = screen.split(largest.id, axis, at, Part::After, min)?;
    next.set_editor(id, Panel::Properties).then_some(next)
}

/// The views of the areas showing a panel, kept as long as the area exists.
#[derive(Default)]
struct AreaViews {
    previews: HashMap<AreaId, (Entity<PreviewArea>, Entity<PreviewToolbar>)>,
    timelines: HashMap<AreaId, Entity<TimelineBar>>,
    code: HashMap<AreaId, Entity<CodeView>>,
    assets: HashMap<AreaId, Entity<AssetsPanel>>,
    graphs: HashMap<AreaId, Entity<Editor>>,
    breadcrumbs: HashMap<AreaId, Entity<Breadcrumbs>>,
    /// Where the graph areas of a loaded layout were, until their editors are made.
    pending: HashMap<AreaId, GraphView>,
    /// Whether the shell's own editor is shown in an area: the others edit the same project through their own.
    primary_shown: bool,
    /// The zoom the first graph area opens at, from `--zoom`.
    startup_zoom: Option<f32>,
}

/// The window: the screen of areas, hosting the graph editor, the previews and the code.
pub struct Shell {
    screen: Entity<ScreenView<Panel>>,
    playback: Entity<Playback>,
    editor: Entity<Editor>,
    /// The Properties area's panel, shared by every area showing it.
    properties: Entity<PropertiesPanel>,
    views: Rc<RefCell<AreaViews>>,
    path: Option<PathBuf>,
    /// Whether a discard prompt is open, so repeated close or quit requests don't stack prompts.
    confirming: bool,
}

/// What a new window opens with.
pub enum Start {
    /// What the app starts with: the project asked for, else the last one opened.
    Launch(Option<PathBuf>),
    Blank,
    /// A project already read, which the window takes over.
    Loaded(PathBuf, Box<Loaded>),
}

impl Shell {
    /// Starts playback, as `--play` asks for at launch.
    pub fn play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.playback.update(cx, |playback, cx| playback.toggle(window, cx));
    }

    /// `zoom` opens the first graph area at that zoom, as `--zoom` asks for at launch.
    #[cfg(test)]
    pub fn new(requested: Option<PathBuf>, zoom: Option<f32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::start(Start::Launch(requested), zoom, window, cx)
    }

    pub fn start(start: Start, zoom: Option<f32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.new(|_| Playback::new());
        let editor = cx.new(|cx| Editor::new(window, cx));
        editor.update(cx, |e, cx| e.attach_playback(&playback, cx));
        let mut path = None;
        let mut layout = None;
        let mut stored_output = None;
        let opening = match start {
            Start::Launch(requested) => requested.or_else(project::last).and_then(|last| match project::read(&last) {
                Ok(loaded) => Some((last, loaded)),
                Err(e) => {
                    log::warn!("couldn't reopen {}: {e:#}", last.display());
                    None
                }
            }),
            Start::Blank => None,
            Start::Loaded(path, loaded) => {
                project::remember(Some(&path));
                Some((path, *loaded))
            }
        };
        if let Some((last, Loaded { project: loaded, files })) = opening {
            layout = layout::load(&loaded);
            stored_output = video_export::stored(&loaded);
            assets::report_problems(window, &unusable_files(&loaded, &files), cx);
            playback.update(cx, |p, cx| p.set_asset_files(files, window, cx));
            editor.update(cx, |e, cx| e.load_project(Some(loaded), None, cx));
            path = Some(last);
        }

        playback.update(cx, |p, cx| apply_settings(p, &editor.read(cx).settings(), window, cx));
        let first = editor.read(cx).shader().clone();
        playback.update(cx, |p, cx| {
            p.set_source(first, window, cx)
        });
        cx.subscribe_in(&editor, window, {
            let playback = playback.clone();
            move |this, _, event: &EditorEvent, window, cx| {
                if let EditorEvent::ExportGroup = event {
                    guard::guard("export group", || this.export_selected_group(window, cx));
                    return;
                }
                playback.update(cx, |p, cx| {
                    guard::guard("playback update", || match event {
                        EditorEvent::Shader(shader) => p.set_source(shader.clone(), window, cx),
                        EditorEvent::Uniforms(values) => p.set_uniforms(values, window, cx),
                        EditorEvent::Settings(settings) => apply_settings(p, settings, window, cx),
                        EditorEvent::Edited | EditorEvent::ExportGroup => {}
                    })
                });
                this.refresh_title(window, cx);
            }
        })
        .detach();

        cx.subscribe_in(&playback, window, |this, playback, event: &PlaybackEvent, window, cx| {
            let PlaybackEvent::Changed = event;
            guard::guard("playback settings", || this.adopt_playback_settings(playback, cx));
            // The menus show the focused window's playback.
            if window.is_window_active() {
                let playing = playback.read(cx).playing();
                crate::set_playing(cx, playing);
            }
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let playing = this.playback.read(cx).playing();
                crate::set_playing(cx, playing);
            }
        })
        .detach();

        let output = video_export::from_stored(stored_output.as_ref(), editor.read(cx).settings().timing.fps);
        let properties = cx.new(|cx| PropertiesPanel::new(editor.clone(), playback.clone(), output, cx));
        let views = Rc::new(RefCell::new(AreaViews { startup_zoom: zoom, ..Default::default() }));
        if let Some(layout) = &layout {
            views.borrow_mut().pending.extend(layout.graphs.iter().cloned());
        }
        let screen = cx.new(|cx| {
            let playback = playback.clone();
            let editor = editor.clone();
            let views = views.clone();
            let properties = properties.clone();
            ScreenView::new(layout.map_or_else(default_screen, |l| l.screen), move |id, panel, window, cx| {
                let mut views = views.borrow_mut();
                match panel {
                    Panel::Graph => {
                        let area = match views.graphs.get(&id).cloned() {
                            Some(area) => area,
                            None => {
                                let area = if views.primary_shown {
                                    let share = editor.read(cx).share();
                                    cx.new(|cx| Editor::another_area(share, window, cx))
                                } else {
                                    views.primary_shown = true;
                                    editor.clone()
                                };
                                if let Some(view) = views.pending.remove(&id) {
                                    area.update(cx, |e, cx| e.set_view(&view, cx));
                                }
                                if views.graphs.is_empty()
                                    && let Some(zoom) = views.startup_zoom
                                {
                                    area.update(cx, |e, cx| e.fit_at_zoom(zoom, cx));
                                }
                                views.graphs.insert(id, area.clone());
                                area
                            }
                        };
                        let crumbs = views
                            .breadcrumbs
                            .entry(id)
                            .or_insert_with(|| cx.new(|cx| Breadcrumbs::new(area.clone(), cx)))
                            .clone();
                        AreaContent { toolbar: Some(crumbs.into()), body: Some(area.into()) }
                    }
                    Panel::Preview => {
                        let (area, toolbar) = views
                            .previews
                            .entry(id)
                            .or_insert_with(|| preview::new_area(&playback, cx))
                            .clone();
                        AreaContent {
                            toolbar: Some(toolbar.into()),
                            body: Some(area.into()),
                        }
                    }
                    Panel::Timeline => {
                        let bar = views
                            .timelines
                            .entry(id)
                            .or_insert_with(|| {
                                cx.new(|cx| TimelineBar::new(playback.clone(), window, cx))
                            })
                            .clone();
                        AreaContent { toolbar: Some(bar.into()), body: None }
                    }
                    Panel::Assets => {
                        let panel = views
                            .assets
                            .entry(id)
                            .or_insert_with(|| cx.new(|cx| AssetsPanel::new(editor.clone(), playback.clone(), window, cx)))
                            .clone();
                        AreaContent { toolbar: None, body: Some(panel.into()) }
                    }
                    Panel::Properties => AreaContent { toolbar: None, body: Some(properties.clone().into()) },
                    Panel::Code => AreaContent {
                        toolbar: None,
                        body: Some(
                            views
                                .code
                                .entry(id)
                                .or_insert_with(|| code_view::new(&playback, cx))
                                .clone()
                                .into(),
                        ),
                    },
                }
            }, cx)
        });
        let id = cx.entity_id();
        let mut shell = Self { screen, playback, editor, properties, views, path, confirming: false };
        shell.refresh_title(window, cx);
        cx.on_release(move |_, cx| windows::owner_closed(id, cx)).detach();
        let shell_weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            shell_weak.update(cx, |this, cx| this.close_requested(window, cx)).unwrap_or(true)
        });
        shell
    }

    /// Whether the window may close now: with unsaved changes it closes only once they are discarded.
    fn close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.editor.read(cx).has_unsaved_changes() {
            return true;
        }
        self.confirm_discard(window, cx, |_, window, _| window.remove_window());
        false
    }

    /// The preview areas the pointer is over.
    fn previews_under_pointer(&self, window: &Window, cx: &App) -> Vec<Entity<PreviewArea>> {
        self.views
            .borrow()
            .previews
            .values()
            .map(|(area, _)| area.clone())
            .filter(|area| area.read(cx).pointer_over(window))
            .collect()
    }

    /// Applies `f` to the preview areas the pointer is over. Keys reach the shell whatever has focus,
    /// so the pointer decides which preview they act on.
    fn over_previews(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        f: fn(&mut PreviewArea, &mut Context<PreviewArea>),
    ) {
        for area in self.previews_under_pointer(window, cx) {
            area.update(cx, f);
        }
    }

    /// Applies `f` for a Display menu item. The pointer is on the menu bar when one is chosen, so with no
    /// preview under the pointer it applies to every preview.
    fn menu_previews(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        f: fn(&mut PreviewArea, &mut Context<PreviewArea>),
    ) {
        let hovered = self.previews_under_pointer(window, cx);
        let targets = if hovered.is_empty() {
            self.views.borrow().previews.values().map(|(area, _)| area.clone()).collect()
        } else {
            hovered
        };
        for area in targets {
            area.update(cx, f);
        }
    }

    /// The preview's size and the timeline's timing are project settings too: edits made there are kept.
    fn adopt_playback_settings(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        let (resolution, timing) = {
            let p = playback.read(cx);
            (p.resolution, p.timing())
        };
        let mut settings = self.editor.read(cx).settings();
        if !settings.canvas.follow_preview {
            settings.canvas.width = resolution.width;
            settings.canvas.height = resolution.height;
        }
        settings.timing.fps = timing.fps;
        settings.timing.realtime = timing.realtime;
        if settings != self.editor.read(cx).settings() {
            self.editor.update(cx, |e, cx| e.update_settings(settings, cx));
        }
    }

    /// Shows the Properties area on `section`, or on the section it was last on. A window without one gets
    /// one, split from its largest area.
    pub(crate) fn reveal_properties(&mut self, section: Option<Section>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(section) = section {
            self.properties.update(cx, |p, cx| p.show(section, cx));
        }
        let screen = self.screen.read(cx).screen().clone();
        if screen.areas().iter().all(|area| area.editor != Panel::Properties) {
            let Some(next) = split_for_properties(&screen, window) else {
                log::warn!("there is no room in the window for the Properties area");
                return;
            };
            self.screen.update(cx, |s, cx| s.set_screen(next, cx));
        }
        let focus = self.properties.focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Starts the video export with the Output section's settings, showing that section.
    fn export_video(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_properties(Some(Section::Output), window, cx);
        self.properties.update(cx, |p, cx| p.start_export(window, cx));
    }

    /// Opens an output window for this project; `window` is the window that draws it.
    pub(crate) fn new_output(&mut self, window: AnyWindowHandle, cx: &mut Context<Self>) {
        if let Err(error) = output::open(&cx.entity(), &self.playback, window, cx) {
            log::error!("failed to open the output window: {error:#}");
        }
    }

    #[cfg(test)]
    pub(crate) fn editor(&self) -> &Entity<Editor> {
        &self.editor
    }

    #[cfg(test)]
    pub(crate) fn playback(&self) -> &Entity<Playback> {
        &self.playback
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether this is a new project nothing was done to, which a project opened or created may replace.
    fn untouched(&self, cx: &App) -> bool {
        self.path.is_none() && !self.editor.read(cx).has_unsaved_changes()
    }

    fn project_name(&self) -> String {
        self.path.as_deref().map_or_else(|| "Untitled".to_owned(), project::file_name)
    }

    fn refresh_title(&mut self, window: &mut Window, cx: &mut App) {
        let name = self.project_name();
        let dirty = self.editor.read(cx).has_unsaved_changes();
        window.set_window_title(&format!("{name}{} - webgpu-nodes", if dirty { " *" } else { "" }));
        window.set_window_edited(dirty);
    }

    fn report(window: &mut Window, message: &str, error: &anyhow::Error, cx: &mut App) {
        log::error!("{message}: {error:#}");
        let detail = format!("{error:#}");
        drop(window.prompt(PromptLevel::Critical, message, Some(&detail), &["OK"], cx));
    }

    /// Runs `then` now, or once the user agrees to lose unsaved changes.
    fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.editor.read(cx).has_unsaved_changes() {
            then(self, window, cx);
            return;
        }
        if self.confirming {
            return;
        }
        self.confirming = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some("The project has changes that haven't been saved."),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let discard = answer.await == Ok(0);
            this.update_in(cx, |this, window, cx| {
                this.confirming = false;
                if discard {
                    guard::guard("discard changes", || then(this, window, cx));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Replaces the areas; the graph areas get editors of their own again.
    fn show_screen(&mut self, screen: Screen<Panel>, graphs: Vec<(AreaId, GraphView)>, cx: &mut Context<Self>) {
        {
            let mut views = self.views.borrow_mut();
            views.graphs.clear();
            views.breadcrumbs.clear();
            views.primary_shown = false;
            views.pending = graphs.into_iter().collect();
        }
        self.screen.update(cx, |s, cx| s.set_screen(screen, cx));
    }

    fn adopt(&mut self, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        project::remember(path.as_deref());
        self.path = path;
        self.refresh_title(window, cx);
    }

    /// Opens a new project in a new window, or stays here when this one is a new project already.
    fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.untouched(cx) {
            return;
        }
        project::remember(None);
        self.open_window(Start::Blank, window, cx);
    }

    fn open_window(&mut self, start: Start, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = windows::open_project(start, None, Some(window.bounds()), None, cx) {
            Self::report(window, "Couldn't open a window", &e, cx);
        }
    }

    fn open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    log::error!("the open dialog failed: {e:#}");
                    None
                }
                _ => None,
            };
            if let Some(path) = picked {
                this.update_in(cx, |this, window, cx| {
                    guard::guard("open project", || this.open_path(path, window, cx));
                })
                .ok();
            }
        })
        .detach();
    }

    /// Opens a project file: a window that has it open comes to the front, a new project here is
    /// replaced by it, and otherwise it opens in a new window.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.as_deref() == Some(path.as_path()) {
            window.activate_window();
            return;
        }
        if let Some(open) = windows::find_project(&path, cx.entity_id(), cx) {
            windows::activate(open, cx);
            return;
        }
        if self.untouched(cx) {
            self.load(path, window, cx);
            return;
        }
        match project::read(&path) {
            Ok(loaded) => self.open_window(Start::Loaded(path, Box::new(loaded)), window, cx),
            Err(e) => {
                let message = format!("Couldn't open {}", project::file_name(&path));
                Self::report(window, &message, &e, cx);
            }
        }
    }

    pub fn load(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match project::read(&path) {
            Ok(Loaded { project: loaded, files }) => {
                let layout = layout::load(&loaded);
                let stored = video_export::stored(&loaded);
                assets::report_problems(window, &unusable_files(&loaded, &files), cx);
                self.playback.update(cx, |p, cx| p.set_asset_files(files, window, cx));
                self.editor.update(cx, |e, cx| e.load_project(Some(loaded), None, cx));
                let fps = self.editor.read(cx).settings().timing.fps;
                let output = video_export::from_stored(stored.as_ref(), fps);
                self.properties.update(cx, |p, cx| p.set_output(output, cx));
                let (screen, graphs) = layout.map_or_else(|| (default_screen(), Vec::new()), |l| (l.screen, l.graphs));
                self.show_screen(screen, graphs, cx);
                self.adopt(Some(path), window, cx);
            }
            Err(e) => {
                let message = format!("Couldn't open {}", project::file_name(&path));
                Self::report(window, &message, &e, cx);
            }
        }
    }

    fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.path.clone() {
            Some(path) => self.save_to(path, window, cx),
            None => self.save_project_as(window, cx),
        }
    }

    /// Asks where to save the selected group's groups bundle, named after the group.
    fn export_selected_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reference) = self.editor.read(cx).selected_group_ref() else { return };
        let export = self.editor.read(cx).model().export_group(&reference);
        let Some(export) = export else { return };
        let files = self.playback.read(cx).asset_files().clone();
        let bytes = match group_bundles::encode(&export, &files) {
            Ok(bytes) => bytes,
            Err(e) => return Self::report(window, "Couldn't export the group", &e, cx),
        };
        let directory = dirs_home().unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!("{}.{}", export.name, BundleKind::Groups.extension());
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let picked = match chosen.await {
                Ok(Ok(path)) => path,
                Ok(Err(e)) => {
                    log::error!("the save dialog failed: {e:#}");
                    None
                }
                Err(_) => None,
            };
            if let Some(path) = picked {
                this.update_in(cx, |_, window, cx| {
                    if let Err(e) = group_bundles::write(&path, &bytes) {
                        let message = format!("Couldn't save {}", project::file_name(&path));
                        Self::report(window, &message, &e, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn save_project_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(dirs_home)
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = self.path.as_deref().map_or_else(
            || format!("untitled.{}", project::EXTENSION),
            project::file_name,
        );
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let picked = match chosen.await {
                Ok(Ok(path)) => path,
                Ok(Err(e)) => {
                    log::error!("the save dialog failed: {e:#}");
                    None
                }
                Err(_) => None,
            };
            if let Some(path) = picked {
                this.update_in(cx, |this, window, cx| {
                    guard::guard("save project", || this.save_to(path, window, cx));
                })
                .ok();
            }
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let mut project = editor.project(cx);
        {
            let views = self.views.borrow();
            let graph = |id: AreaId| views.graphs.get(&id).map(|e| e.read(cx).view(cx)).unwrap_or_default();
            layout::store(&mut project, self.screen.read(cx).screen(), &graph);
        }
        video_export::store(&mut project, &self.properties.read(cx).output());
        let files = self.playback.read(cx).asset_files().clone();
        let result: Result<()> = project::write(&path, &project, &files);
        match result {
            Ok(()) => {
                self.editor.read(cx).mark_saved();
                self.adopt(Some(path), window, cx);
            }
            Err(e) => {
                let message = format!("Couldn't save {}", project::file_name(&path));
                Self::report(window, &message, &e, cx);
            }
        }
    }
}

fn apply_settings(
    playback: &mut Playback,
    settings: &ProjectSettings,
    window: &mut Window,
    cx: &mut Context<Playback>,
) {
    let canvas = settings.canvas;
    playback.set_canvas(canvas.follow_preview, Size::new(canvas.width, canvas.height), window, cx);
    let timing = Timing { fps: settings.timing.fps, realtime: settings.timing.realtime };
    if timing != playback.timing() {
        playback.set_timing(timing, window, cx);
    }
}

/// The names of the assets whose files the project's bundle doesn't have, or that don't match them.
fn unusable_files(project: &GraphProject, files: &AssetFiles) -> Vec<String> {
    project
        .assets
        .iter()
        .filter(|asset| !files.contains_key(asset.id()))
        .map(|asset| asset.name().to_owned())
        .collect()
}

pub fn request_quit(cx: &mut App) {
    confirm_all_discards(cx, |cx| cx.quit());
}

/// Runs `then` once every project window has agreed to lose its unsaved changes, asking each in turn.
/// A window that refuses stops it.
pub(crate) fn confirm_all_discards(cx: &mut App, then: impl FnOnce(&mut App) + 'static) {
    let windows = windows::project_windows(cx).into_iter().map(|(handle, _)| handle).collect();
    ask_each(windows, Box::new(then), cx);
}

fn ask_each(mut remaining: Vec<AnyWindowHandle>, then: Box<dyn FnOnce(&mut App)>, cx: &mut App) {
    let Some(handle) = remaining.pop() else {
        then(cx);
        return;
    };
    let asked = cx.update_window(handle, |view, window, cx| {
        let Some(shell) = windows::shell_of_view(&view, cx) else { return };
        shell.update(cx, |shell, cx| {
            if shell.editor.read(cx).has_unsaved_changes() {
                window.activate_window();
            }
            shell.confirm_discard(window, cx, move |_, _, cx| ask_each(remaining, then, cx));
        });
    });
    if let Err(error) = asked {
        log::error!("couldn't ask about unsaved changes: {error:#}");
    }
}

fn dirs_home() -> Option<PathBuf> {
    dirs::document_dir().or_else(dirs::home_dir)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("shell", || {
        preferences::apply(window, cx);
        let has_preview = !self.views.borrow().previews.is_empty();
        let theme = cx.theme();
        div()
            .key_context("Shell")
            .size_full()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.font.clone())
            .on_action(cx.safe_listener(|this, _: &TogglePlayback, window, cx| {
                let playback = this.playback.clone();
                playback.update(cx, |p, cx| p.toggle(window, cx));
            }))
            .on_action(cx.safe_listener(|this, _: &PreviousFrame, window, cx| {
                this.playback.update(cx, |p, cx| p.step(-1, window, cx));
            }))
            .on_action(cx.safe_listener(|this, _: &NextFrame, window, cx| {
                this.playback.update(cx, |p, cx| p.step(1, window, cx));
            }))
            .on_action(cx.safe_listener(|this, _: &GoToStart, window, cx| {
                this.playback.update(cx, |p, cx| p.seek(0.0, window, cx));
            }))
            .on_action(cx.safe_listener(|this, _: &NewProject, window, cx| {
                this.new_project(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &OpenProject, window, cx| {
                this.open_project(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &SaveProject, window, cx| {
                this.save_project(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &SaveProjectAs, window, cx| {
                this.save_project_as(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &ImportMedia, window, cx| {
                assets::pick_and_import(&this.editor, &this.playback, window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &ImportGroups, window, cx| {
                group_bundles::pick_and_import(&this.editor, &this.playback, window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &ExportVideo, window, cx| {
                this.export_video(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &OpenProjectSettings, window, cx| {
                this.reveal_properties(None, window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &NewOutputWindow, window, cx| {
                this.new_output(window.window_handle(), cx);
            }))
            .on_action(cx.safe_listener(|this, _: &CloseWindow, window, cx| {
                if this.close_requested(window, cx) {
                    window.remove_window();
                }
            }))
            .on_action(cx.safe_listener(|this, _: &ExportFrame, window, cx| {
                preview::export_png(&this.playback, window, cx);
            }))
            .on_action(cx.safe_listener(|_, _: &ToggleMode, window, cx| {
                preferences::toggle_mode(window, cx);
            }))
            .when(has_preview, |d| {
                d.on_action(cx.safe_listener(|this, _: &ZoomPreviewIn, window, cx| {
                    this.over_previews(window, cx, PreviewArea::zoom_in);
                }))
                .on_action(cx.safe_listener(|this, _: &ZoomPreviewOut, window, cx| {
                    this.over_previews(window, cx, PreviewArea::zoom_out);
                }))
                .on_action(cx.safe_listener(|this, _: &FitPreview, window, cx| {
                    this.over_previews(window, cx, PreviewArea::fit);
                }))
                .on_action(cx.safe_listener(|this, _: &ActualSizePreview, window, cx| {
                    this.over_previews(window, cx, PreviewArea::actual_size);
                }))
                .on_action(cx.safe_listener(|this, _: &MenuZoomPreviewIn, window, cx| {
                    this.menu_previews(window, cx, PreviewArea::zoom_in);
                }))
                .on_action(cx.safe_listener(|this, _: &MenuZoomPreviewOut, window, cx| {
                    this.menu_previews(window, cx, PreviewArea::zoom_out);
                }))
                .on_action(cx.safe_listener(|this, _: &MenuFitPreview, window, cx| {
                    this.menu_previews(window, cx, PreviewArea::fit);
                }))
                .on_action(cx.safe_listener(|this, _: &MenuActualSizePreview, window, cx| {
                    this.menu_previews(window, cx, PreviewArea::actual_size);
                }))
            })
            .on_drop(cx.safe_listener(|this, paths: &ExternalPaths, window, cx| {
                assets::import_paths(&this.editor, &this.playback, &paths.0, window, cx);
            }))
            .child(self.screen.clone().cached(StyleRefinement::default().size_full()))
    })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{Modifiers, TestAppContext};
    use shader_graph::assets::{Asset, AssetFile, asset_hash};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wgnodes-shell-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A shell that starts without a project file, with the app's keys bound.
    fn open(cx: &mut TestAppContext) -> (Entity<Shell>, gpui::VisualTestContext) {
        cx.update(|cx| {
            theme::init(cx);
            preferences::init(cx);
            init(cx);
        });
        let missing = PathBuf::from("/nonexistent/untitled.wgnodes");
        let (shell, cx) = cx.add_window_view(|window, cx| Shell::new(Some(missing), None, window, cx));
        cx.run_until_parked();
        (shell, cx.clone())
    }

    fn dirty(shell: &Entity<Shell>, cx: &mut gpui::VisualTestContext) {
        shell.update(cx, |shell, cx| {
            shell.editor.update(cx, |editor, cx| {
                let mut settings = editor.settings();
                settings.canvas.width += 1;
                editor.update_settings(settings, cx);
            });
        });
        assert!(shell.read_with(cx, |s, cx| s.editor.read(cx).has_unsaved_changes()));
    }

    fn preview_area(shell: &Entity<Shell>, cx: &mut gpui::VisualTestContext) -> Entity<PreviewArea> {
        shell.read_with(cx, |s, _| {
            s.views.borrow().previews.values().next().map(|(area, _)| area.clone()).expect("a preview area")
        })
    }

    fn open_windows(cx: &mut gpui::VisualTestContext) -> usize {
        cx.cx.update(|cx| cx.windows().len())
    }

    #[gpui::test]
    fn zoom_keys_act_on_the_preview_under_the_pointer(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        let area = preview_area(&shell, cx);
        let zoom = |cx: &mut gpui::VisualTestContext| area.read_with(cx, |a, cx| a.zoom(cx));
        let before = zoom(cx);

        cx.simulate_keystrokes("+");
        assert_eq!(zoom(cx), before, "the keys do nothing while the pointer is elsewhere");

        let pane = cx.debug_bounds("preview-pane").expect("the preview pane is drawn");
        cx.simulate_mouse_move(pane.center(), None, Modifiers::none());
        cx.run_until_parked();

        cx.simulate_keystrokes("+");
        assert!((zoom(cx) - before * 1.2).abs() < 1e-4, "+ zooms in by one step");
        cx.simulate_keystrokes("=");
        assert!((zoom(cx) - before * 1.2 * 1.2).abs() < 1e-3, "= zooms in too");
        cx.simulate_keystrokes("-");
        assert!((zoom(cx) - before * 1.2).abs() < 1e-3, "- zooms out by one step");
        cx.simulate_keystrokes("-");
        assert!((zoom(cx) - before).abs() < 1e-3, "a second - returns to the start");
        cx.simulate_keystrokes("1");
        assert!((zoom(cx) - 1.0).abs() < 1e-6, "1 shows the canvas at actual size");
        cx.simulate_keystrokes("0");
        assert!((zoom(cx) - before).abs() < 1e-4, "0 fits the canvas again");

        cx.simulate_keystrokes("cmd-+");
        assert!((zoom(cx) - before).abs() < 1e-4, "keys with modifiers aren't zoom keys");
    }

    #[gpui::test]
    fn cmd_m_switches_between_light_and_dark(cx: &mut TestAppContext) {
        let (_, mut cx) = open(cx);
        let cx = &mut cx;
        let theme = |cx: &mut gpui::VisualTestContext| cx.read(|cx| cx.global::<preferences::Preferences>().theme);
        cx.simulate_keystrokes("cmd-m");
        assert_eq!(theme(cx), preferences::ThemePreference::Dark);
        cx.simulate_keystrokes("cmd-m");
        assert_eq!(theme(cx), preferences::ThemePreference::Light);
    }

    #[gpui::test]
    fn closing_with_unsaved_changes_asks_first(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        dirty(&shell, cx);

        assert!(!cx.simulate_close(), "the window stays open while the prompt is up");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(open_windows(cx), 1, "cancel keeps the window open");

        assert!(!cx.simulate_close());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert_eq!(open_windows(cx), 0, "discarding closes the window");
    }

    #[gpui::test]
    fn closing_a_saved_project_does_not_ask(cx: &mut TestAppContext) {
        let (_, mut cx) = open(cx);
        let cx = &mut cx;
        assert!(cx.simulate_close());
        assert!(!cx.has_pending_prompt());
    }

    #[gpui::test]
    fn quitting_with_unsaved_changes_asks_first(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        dirty(&shell, cx);

        cx.cx.update(request_quit);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(open_windows(cx), 1);
    }

    #[gpui::test]
    fn a_project_missing_files_reports_them_in_one_dialog(cx: &mut TestAppContext) {
        let dir = scratch("missing");
        let path = dir.join("project.wgnodes");
        let mut project = GraphProject::default();
        for (id, name) in [("a1", "First"), ("a2", "Second")] {
            project.assets.push(Asset::Image(AssetFile {
                id: id.into(),
                name: name.into(),
                file: format!("assets/images/{id}.png"),
                mime: "image/png".into(),
                bytes: 3,
                hash: asset_hash(&[1, 2, 3]),
                width: 1,
                height: 1,
            }));
        }
        project::write(&path, &project, &AssetFiles::new()).expect("writes");

        cx.update(|cx| {
            theme::init(cx);
            preferences::init(cx);
            init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| Shell::new(Some(path), None, window, cx));
        cx.run_until_parked();
        let (title, detail) = cx.pending_prompt().expect("the problems are reported");
        assert_eq!(title, "Some files couldn't be used");
        assert!(detail.contains("First") && detail.contains("Second"), "{detail}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn assets_without_their_files_are_the_unusable_ones() {
        let mut project = GraphProject::default();
        let asset = |id: &str| {
            Asset::Image(AssetFile {
                id: id.into(),
                name: format!("{id}-name"),
                file: format!("assets/images/{id}.png"),
                mime: "image/png".into(),
                bytes: 1,
                hash: asset_hash(&[0]),
                width: 1,
                height: 1,
            })
        };
        project.assets.extend([asset("a1"), asset("a2")]);
        let files = AssetFiles::from([("a1".to_owned(), std::sync::Arc::new(vec![0]))]);
        assert_eq!(unusable_files(&project, &files), ["a2-name"]);
        assert!(unusable_files(&project, &AssetFiles::from([
            ("a1".to_owned(), std::sync::Arc::new(vec![0])),
            ("a2".to_owned(), std::sync::Arc::new(vec![0])),
        ]))
        .is_empty());
    }

    #[gpui::test]
    fn the_display_menu_zooms_the_preview_with_the_pointer_elsewhere(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        let area = preview_area(&shell, cx);
        let zoom = |cx: &mut gpui::VisualTestContext| area.read_with(cx, |a, cx| a.zoom(cx));
        let before = zoom(cx);
        let focus = shell.read_with(cx, |s, cx| s.editor.read(cx).focus_handle(cx));
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.run_until_parked();

        cx.dispatch_action(MenuZoomPreviewIn);
        cx.run_until_parked();
        assert!((zoom(cx) - before * 1.2).abs() < 1e-4, "Zoom preview in zooms in by one step");
        cx.dispatch_action(MenuActualSizePreview);
        cx.run_until_parked();
        assert!((zoom(cx) - 1.0).abs() < 1e-6, "Preview at actual size shows the canvas at 100%");
    }

    #[gpui::test]
    fn developer_tools_toggle_the_preference_and_its_menu_item(cx: &mut TestAppContext) {
        let (_shell, mut cx) = open(cx);
        let before = cx.update(|_, cx| cx.global::<Preferences>().developer_tools_enabled());
        cx.update(|_, cx| cx.dispatch_action(&preferences::ToggleDeveloperTools));
        assert_eq!(cx.update(|_, cx| cx.global::<Preferences>().developer_tools_enabled()), !before);

        let checked = cx.update(|_, cx| {
            cx.get_menus()
                .into_iter()
                .flatten()
                .find(|m| m.name.as_ref() == "File")
                .and_then(|m| m.items.into_iter().last())
                .is_some_and(|item| matches!(item, gpui::OwnedMenuItem::Action { checked: true, .. }))
        });
        assert_eq!(checked, !before, "the Developer tools item shows the new state");
    }

    fn properties_areas(shell: &Entity<Shell>, cx: &mut gpui::VisualTestContext) -> Vec<Panel> {
        shell.read_with(cx, |s, cx| s.screen.read(cx).screen().areas().iter().map(|a| a.editor).collect())
    }

    #[gpui::test]
    fn the_default_layout_has_a_properties_area_that_project_settings_focus(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        let count = |areas: Vec<Panel>| areas.iter().filter(|p| **p == Panel::Properties).count();
        assert_eq!(count(properties_areas(&shell, cx)), 1);

        cx.dispatch_action(OpenProjectSettings);
        cx.run_until_parked();
        assert_eq!(count(properties_areas(&shell, cx)), 1, "project settings focus the area there is");
        let focus = shell.read_with(cx, |s, cx| s.properties.focus_handle(cx));
        assert!(cx.update(|window, _| focus.is_focused(window)), "and it takes the focus");
    }

    #[gpui::test]
    fn project_settings_open_an_area_in_a_window_without_one(cx: &mut TestAppContext) {
        let (shell, mut cx) = open(cx);
        let cx = &mut cx;
        shell.update(cx, |s, cx| {
            s.screen.update(cx, |view, cx| {
                view.set_screen(Screen::new([(Rect::new(0.0, 0.0, 1.0, 1.0), Panel::Graph)]), cx);
            });
        });
        cx.dispatch_action(OpenProjectSettings);
        cx.run_until_parked();
        let areas = properties_areas(&shell, cx);
        assert_eq!(areas.len(), 2, "the window gets one more area");
        assert!(areas.contains(&Panel::Properties));
    }

    #[gpui::test]
    fn the_output_settings_a_project_saved_come_back_when_it_opens(cx: &mut TestAppContext) {
        let dir = scratch("output");
        let path = dir.join("a.wgnodes");
        let output = video_export::OutputSettings {
            container: export::Container::Matroska,
            codec: export::Codec::Png,
            alpha: true,
            start: 1.0,
            duration: 2.0,
            fps: 24.0,
        };
        let mut project = GraphProject::from_json(&GraphProject::default().to_json()).expect("loads");
        video_export::store(&mut project, &output);
        project::write(&path, &project, &AssetFiles::new()).expect("writes");

        cx.update(|cx| {
            theme::init(cx);
            preferences::init(cx);
            init(cx);
        });
        let (shell, cx) = cx.add_window_view(|window, cx| Shell::new(Some(path), None, window, cx));
        cx.run_until_parked();
        let panel = shell.read_with(cx, |s, _| s.properties.clone());
        assert_eq!(panel.read_with(cx, |p, _| p.output()), output);
        std::fs::remove_dir_all(dir).ok();
    }
}
