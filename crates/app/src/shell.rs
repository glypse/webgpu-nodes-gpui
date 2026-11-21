use guard::SafeListener as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, PathPromptOptions, PromptLevel, Render, Styled as _, StyleRefinement, Window,
    actions, div,
};
use runtime::{Size, Timing};
use screen::{AreaContent, AreaId, Rect, Screen, ScreenView};
use shader_graph::graph::ProjectSettings;
use theme::{ActiveTheme as _, IconName};

use crate::code_view::{self, CodeView};
use crate::editor::{Editor, EditorEvent};
use crate::graph::breadcrumbs::Breadcrumbs;
use crate::layout::{self, GraphView};
use crate::playback::{Playback, PlaybackEvent};
use crate::preferences;
use crate::settings::{SettingsDialog, SettingsEvent};
use crate::project::{self, Loaded};
use crate::preview::{self, PreviewArea, PreviewToolbar};
use crate::timeline::TimelineBar;

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
        Undo,
        Redo,
        OpenSettings,
        ToggleMode,
        ZoomPreviewIn,
        ZoomPreviewOut,
        FitPreview,
        ActualSizePreview,
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
        KeyBinding::new("cmd-,", OpenSettings, Some("Shell")),
        KeyBinding::new("cmd-m", ToggleMode, Some("Shell")),
        // Both keys of "+": typed with shift on US layouts, where only the typed character matches.
        KeyBinding::new("+", ZoomPreviewIn, Some(CONTEXT)),
        KeyBinding::new("=", ZoomPreviewIn, Some(CONTEXT)),
        KeyBinding::new("-", ZoomPreviewOut, Some(CONTEXT)),
        KeyBinding::new("0", FitPreview, Some(CONTEXT)),
        KeyBinding::new("1", ActualSizePreview, Some(CONTEXT)),
    ]);
}

/// The kinds of panel an area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Graph,
    Preview,
    Timeline,
    Code,
}

impl screen::Editor for Panel {
    fn all() -> &'static [Self] {
        &[Self::Graph, Self::Preview, Self::Timeline, Self::Code]
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Graph => "Node graph",
            Self::Preview => "Preview",
            Self::Timeline => "Timeline",
            Self::Code => "WGSL",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Graph => IconName::TreeStructure,
            Self::Preview => IconName::MonitorPlay,
            Self::Timeline => IconName::FilmStrip,
            Self::Code => IconName::Code,
        }
    }
}

fn default_screen() -> Screen<Panel> {
    Screen::new([
        (Rect::new(0.0, 0.0, 0.6, 1.0), Panel::Graph),
        (Rect::new(0.6, 0.0, 1.0, 0.55), Panel::Preview),
        (Rect::new(0.6, 0.55, 1.0, 0.63), Panel::Timeline),
        (Rect::new(0.6, 0.63, 1.0, 1.0), Panel::Code),
    ])
}

/// The views of the areas showing a panel, kept as long as the area exists.
#[derive(Default)]
struct AreaViews {
    previews: HashMap<AreaId, (Entity<PreviewArea>, Entity<PreviewToolbar>)>,
    timelines: HashMap<AreaId, Entity<TimelineBar>>,
    code: HashMap<AreaId, Entity<CodeView>>,
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
    views: Rc<RefCell<AreaViews>>,
    path: Option<PathBuf>,
    settings: Option<Entity<SettingsDialog>>,
}

impl Shell {
    /// Starts playback, as `--play` asks for at launch.
    pub fn play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.playback.update(cx, |playback, cx| playback.toggle(window, cx));
    }

    /// `zoom` opens the first graph area at that zoom, as `--zoom` asks for at launch.
    pub fn new(requested: Option<PathBuf>, zoom: Option<f32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.new(|_| Playback::new());
        let editor = cx.new(|cx| Editor::new(window, cx));
        let mut path = None;
        let mut layout = None;
        if let Some(last) = requested.or_else(project::last) {
            match project::read(&last) {
                Ok(Loaded { project: loaded, files }) => {
                    layout = layout::load(&loaded);
                    editor.update(cx, |e, cx| e.load_project(Some(loaded), None, cx));
                    playback.update(cx, |p, cx| p.set_asset_files(files, window, cx));
                    path = Some(last);
                }
                Err(e) => log::warn!("couldn't reopen {}: {e:#}", last.display()),
            }
        }

        playback.update(cx, |p, cx| apply_settings(p, &editor.read(cx).settings(), window, cx));
        let first = editor.read(cx).shader().clone();
        playback.update(cx, |p, cx| {
            p.set_source(first, window, cx)
        });
        cx.subscribe_in(&editor, window, {
            let playback = playback.clone();
            move |this, _, event: &EditorEvent, window, cx| {
                playback.update(cx, |p, cx| {
                    guard::guard("playback update", || match event {
                        EditorEvent::Shader(shader) => p.set_source(shader.clone(), window, cx),
                        EditorEvent::Uniforms(values) => p.set_uniforms(values, window, cx),
                        EditorEvent::Settings(settings) => apply_settings(p, settings, window, cx),
                    })
                });
                this.refresh_title(window, cx);
            }
        })
        .detach();

        cx.subscribe_in(&playback, window, |this, playback, event: &PlaybackEvent, _, cx| {
            let PlaybackEvent::Changed = event;
            guard::guard("playback settings", || this.adopt_playback_settings(playback, cx));
        })
        .detach();

        let views = Rc::new(RefCell::new(AreaViews { startup_zoom: zoom, ..Default::default() }));
        if let Some(layout) = &layout {
            views.borrow_mut().pending.extend(layout.graphs.iter().cloned());
        }
        let screen = cx.new(|cx| {
            let playback = playback.clone();
            let editor = editor.clone();
            let views = views.clone();
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
        let shell = Self { screen, playback, editor, views, path, settings: None };
        shell.refresh_title(window, cx);
        shell
    }

    /// Applies `f` to the preview areas the pointer is over. Keys reach the shell whatever has focus,
    /// so the pointer decides which preview they act on.
    fn over_previews(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        f: fn(&mut PreviewArea, &mut Context<PreviewArea>),
    ) {
        let hovered: Vec<_> = self
            .views
            .borrow()
            .previews
            .values()
            .map(|(area, _)| area.clone())
            .filter(|area| area.read(cx).pointer_over(window))
            .collect();
        for area in hovered {
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

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let editor = self.editor.clone();
        let playback = self.playback.clone();
        let dialog = cx.new(|cx| SettingsDialog::new(editor, playback, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, SettingsEvent::Close, window, cx| {
            this.settings = None;
            let editor_focus = this.editor.read(cx).focus_handle(cx);
            window.focus(&editor_focus, cx);
            cx.notify();
        })
        .detach();
        self.settings = Some(dialog);
        cx.notify();
    }

    fn refresh_title(&self, window: &mut Window, cx: &App) {
        let name = self.path.as_deref().map_or_else(|| "Untitled".to_owned(), project::file_name);
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
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some("The project has changes that haven't been saved."),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    guard::guard("discard changes", || then(this, window, cx));
                })
                .ok();
            }
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

    fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, window, cx| {
            this.editor.update(cx, |e, cx| e.load_project(None, None, cx));
            this.show_screen(default_screen(), Vec::new(), cx);
            this.adopt(None, window, cx);
        });
    }

    fn open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |_, window, cx| {
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
                        guard::guard("open project", || this.load(path, window, cx));
                    })
                    .ok();
                }
            })
            .detach();
        });
    }

    pub fn load(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match project::read(&path) {
            Ok(Loaded { project: loaded, files }) => {
                let layout = layout::load(&loaded);
                self.editor.update(cx, |e, cx| e.load_project(Some(loaded), None, cx));
                self.playback.update(cx, |p, cx| p.set_asset_files(files, window, cx));
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

fn dirs_home() -> Option<PathBuf> {
    dirs::document_dir().or_else(dirs::home_dir)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("shell", || {
        preferences::apply(window, cx);
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
            .on_action(cx.safe_listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(window, cx);
            }))
            .on_action(cx.safe_listener(|_, _: &ToggleMode, window, cx| {
                preferences::toggle_mode(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &ZoomPreviewIn, window, cx| {
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
            .on_action(cx.safe_listener(|this, _: &Undo, window, cx| {
                this.editor.update(cx, |e, cx| e.undo(cx));
                this.refresh_title(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &Redo, window, cx| {
                this.editor.update(cx, |e, cx| e.redo(cx));
                this.refresh_title(window, cx);
            }))
            .child(self.screen.clone().cached(StyleRefinement::default().size_full()))
            .children(self.settings.clone())
    })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{Modifiers, TestAppContext};
    
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

    fn preview_area(shell: &Entity<Shell>, cx: &mut gpui::VisualTestContext) -> Entity<PreviewArea> {
        shell.read_with(cx, |s, _| {
            s.views.borrow().previews.values().next().map(|(area, _)| area.clone()).expect("a preview area")
        })
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

}
