use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, Styled as _, Window, actions, div,
};
use screen::{AreaContent, AreaId, Rect, Screen, ScreenView};
use theme::{ActiveTheme as _, IconName, Theme};

use crate::code_view::{self, CodeView};
use crate::editor::{Editor, EditorEvent};
use crate::playback::Playback;
use crate::preview::{self, PreviewArea, PreviewToolbar};
use crate::program::program_source;
use crate::timeline::TimelineBar;

actions!(shell, [TogglePlayback, PreviousFrame, NextFrame, GoToStart]);

/// Playback keys act anywhere but while typing in a field.
const CONTEXT: &str = "Shell && !ThemeTextInput";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, Some(CONTEXT)),
        KeyBinding::new(",", PreviousFrame, Some(CONTEXT)),
        KeyBinding::new(".", NextFrame, Some(CONTEXT)),
        KeyBinding::new("home", GoToStart, Some(CONTEXT)),
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
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.new(|_| Playback::new());
        let editor = cx.new(|cx| Editor::new(window, cx));

        let first = editor.read(cx).shader().clone();
        playback.update(cx, |p, cx| {
            p.set_source(first.map(program_source), window, cx)
        });
        cx.subscribe_in(&editor, window, {
            let playback = playback.clone();
            move |_, _, event: &EditorEvent, window, cx| {
                let EditorEvent::Shader(shader) = event;
                playback.update(cx, |p, cx| {
                    p.set_source(shader.clone().map(program_source), window, cx);
                });
            }
        })
        .detach();

        let views = Rc::new(RefCell::new(AreaViews::default()));
        let screen = cx.new(|_| {
            let playback = playback.clone();
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
        Self { screen, playback }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Theme::sync(window, cx);
        let theme = cx.theme();
        div()
            .key_context("Shell")
            .size_full()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.font.clone())
            .on_action(cx.listener(|this, _: &TogglePlayback, window, cx| {
                let playback = this.playback.clone();
                playback.update(cx, |p, cx| p.toggle(window, cx));
            }))
            .on_action(cx.listener(|this, _: &PreviousFrame, window, cx| {
                this.playback.update(cx, |p, cx| p.step(-1, window, cx));
            }))
            .on_action(cx.listener(|this, _: &NextFrame, window, cx| {
                this.playback.update(cx, |p, cx| p.step(1, window, cx));
            }))
            .on_action(cx.listener(|this, _: &GoToStart, window, cx| {
                this.playback.update(cx, |p, cx| p.seek(0.0, window, cx));
            }))
            .child(self.screen.clone())
    }
}
