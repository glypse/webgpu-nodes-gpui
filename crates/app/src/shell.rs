use guard::SafeListener as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, PathPromptOptions, PromptLevel, Render, Styled as _, Window, actions, div,
};
use runtime::{Size, Timing};
use screen::{AreaContent, AreaId, Rect, Screen, ScreenView};
use shader_graph::graph::ProjectSettings;
use theme::{ActiveTheme as _, IconName};

use crate::code_view::{self, CodeView};
use crate::editor::{Editor, EditorEvent};
use crate::playback::{Playback, PlaybackEvent};
use crate::preferences;
use crate::settings::{SettingsDialog, SettingsEvent};
use crate::project;
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
    ]
);

/// Playback keys act anywhere but while typing in a field.
const CONTEXT: &str = "Shell && !ThemeTextInput";

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
            Self::Timeline => IconName::Play,
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
}

/// The window: the screen of areas, hosting the graph editor, the previews and the code.
pub struct Shell {
    screen: Entity<ScreenView<Panel>>,
    playback: Entity<Playback>,
    editor: Entity<Editor>,
    path: Option<PathBuf>,
    settings: Option<Entity<SettingsDialog>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.new(|_| Playback::new());
        let editor = cx.new(|cx| Editor::new(window, cx));
        let mut path = None;
        if let Some(last) = project::last() {
            match project::read(&last) {
                Ok(loaded) => {
                    editor.update(cx, |e, cx| e.load_project(Some(loaded), cx));
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

        let views = Rc::new(RefCell::new(AreaViews::default()));
        let screen = cx.new(|_| {
            let playback = playback.clone();
            let editor = editor.clone();
            ScreenView::new(default_screen(), move |id, panel, window, cx| {
                let mut views = views.borrow_mut();
                match panel {
                    Panel::Graph => AreaContent {
                        toolbar: None,
                        body: editor.clone().into_any_element(),
                    },
                    Panel::Preview => {
                        let (area, toolbar) = views
                            .previews
                            .entry(id)
                            .or_insert_with(|| preview::new_area(&playback, cx))
                            .clone();
                        AreaContent {
                            toolbar: Some(toolbar.into_any_element()),
                            body: area.into_any_element(),
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
                        AreaContent {
                            toolbar: Some(bar.into_any_element()),
                            body: div().into_any_element(),
                        }
                    }
                    Panel::Code => AreaContent {
                        toolbar: None,
                        body: views
                            .code
                            .entry(id)
                            .or_insert_with(|| code_view::new(&playback, cx))
                            .clone()
                            .into_any_element(),
                    },
                }
            })
        });
        let shell = Self { screen, playback, editor, path, settings: None };
        shell.refresh_title(window, cx);
        shell
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
        let dialog = cx.new(|cx| SettingsDialog::new(editor, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, SettingsEvent::Close, window, cx| {
            this.settings = None;
            window.focus(&this.editor.read(cx).focus_handle(cx));
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

    fn adopt(&mut self, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        project::remember(path.as_deref());
        self.path = path;
        self.refresh_title(window, cx);
    }

    fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, window, cx| {
            this.editor.update(cx, |e, cx| e.load_project(None, cx));
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
            Ok(loaded) => {
                self.editor.update(cx, |e, cx| e.load_project(Some(loaded), cx));
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
        let result: Result<()> = project::write(&path, &self.editor.read(cx).project());
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
    if !settings.canvas.follow_preview {
        playback.set_resolution(Size::new(settings.canvas.width, settings.canvas.height), window, cx);
    }
    let timing = Timing { fps: settings.timing.fps, realtime: settings.timing.realtime };
    if timing != playback.timing() {
        playback.set_timing(timing, window, cx);
    }
}

fn dirs_home() -> Option<PathBuf> {
    dirs::document_dir().or_else(dirs::home_dir)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_action(cx.safe_listener(|this, _: &Undo, window, cx| {
                this.editor.update(cx, |e, cx| e.undo(cx));
                this.refresh_title(window, cx);
            }))
            .on_action(cx.safe_listener(|this, _: &Redo, window, cx| {
                this.editor.update(cx, |e, cx| e.redo(cx));
                this.refresh_title(window, cx);
            }))
            .child(self.screen.clone())
            .children(self.settings.clone())
    }
}
