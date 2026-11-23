//! The app's windows: project windows, each owning its project, the one
//! preferences window and the output windows that belong to a project.

use std::path::Path;

use anyhow::Result;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, EntityId, Global, KeyBinding,
    TitlebarOptions, WindowBounds, WindowOptions, actions, point, px, size,
};
use gpui_component::Root;

use crate::preferences_window::PreferencesDialog;
use crate::shell::{Shell, Start};

actions!(
    windows,
    [
        NewOutputWindow,
        ToggleFullscreen,
        ToggleOutputStretch,
        CloseWindow
    ]
);

/// The size of a project window opened without one, in points.
pub const DEFAULT_SIZE: (f32, f32) = (1400.0, 860.0);
/// How far a new project window sits from the one it opens beside.
const CASCADE: f32 = 28.0;

struct OutputWindow {
    handle: AnyWindowHandle,
    owner: EntityId,
}

#[derive(Default)]
struct Registry {
    preferences: Option<AnyWindowHandle>,
    outputs: Vec<OutputWindow>,
}

impl Global for Registry {}

pub fn init(cx: &mut App) {
    cx.set_global(Registry::default());
    cx.bind_keys([
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
        KeyBinding::new("cmd-alt-o", NewOutputWindow, None),
        KeyBinding::new("cmd-alt-s", ToggleOutputStretch, Some("Output")),
    ]);
    // Actions the focused window doesn't handle end up here. Global listeners run while the focused
    // window is being updated, so they act on windows once that is done.
    cx.on_action(|_: &ToggleFullscreen, cx| {
        cx.defer(|cx| {
            guard::guard("toggle full screen", || {
                if let Some(active) = cx.active_window() {
                    cx.update_window(active, |_, window, _| window.toggle_fullscreen())
                        .ok();
                }
            });
        });
    });
    cx.on_action(|_: &CloseWindow, cx| {
        cx.defer(|cx| {
            guard::guard("close window", || {
                if let Some(active) = cx.active_window()
                    && shell_in(active, cx).is_none()
                {
                    cx.update_window(active, |_, window, _| window.remove_window())
                        .ok();
                }
            });
        });
    });
    // The preferences don't belong to a project, so they open from any window.
    cx.on_action(|_: &crate::shell::OpenPreferences, cx| {
        cx.defer(open_preferences);
    });
}

/// The shell a window's root view is, or the root's content.
pub fn shell_of_view(view: &gpui::AnyView, cx: &App) -> Option<Entity<Shell>> {
    if let Ok(shell) = view.clone().downcast::<Shell>() {
        return Some(shell);
    }
    view.clone()
        .downcast::<Root>()
        .ok()?
        .read(cx)
        .view()
        .clone()
        .downcast::<Shell>()
        .ok()
}

/// The shell a window shows, if it is a project window.
pub fn shell_in(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Shell>> {
    if let Some(root) = handle.downcast::<Root>() {
        return root.read(cx).ok()?.view().clone().downcast::<Shell>().ok();
    }
    handle.downcast::<Shell>()?.entity(cx).ok()
}

/// The project windows. A window being updated right now can't be read, so is left out.
pub fn project_windows(cx: &App) -> Vec<(AnyWindowHandle, Entity<Shell>)> {
    cx.windows()
        .into_iter()
        .filter_map(|handle| Some((handle, shell_in(handle, cx)?)))
        .collect()
}

/// The window of another project window that has `path` open.
pub fn find_project(path: &Path, except: EntityId, cx: &App) -> Option<AnyWindowHandle> {
    project_windows(cx)
        .into_iter()
        .find(|(_, shell)| shell.entity_id() != except && shell.read(cx).path() == Some(path))
        .map(|(handle, _)| handle)
}

pub fn activate(handle: AnyWindowHandle, cx: &mut App) {
    cx.update_window(handle, |_, window, _| window.activate_window())
        .ok();
}

/// Opens a project window beside the one with bounds `beside`, or else centred at `size` (in points).
pub fn open_project(
    start: Start,
    zoom: Option<f32>,
    beside: Option<Bounds<gpui::Pixels>>,
    size: Option<(f32, f32)>,
    cx: &mut App,
) -> Result<gpui::WindowHandle<Root>> {
    let bounds = match beside {
        Some(bounds) => Bounds::new(bounds.origin + point(px(CASCADE), px(CASCADE)), bounds.size),
        None => {
            let (width, height) = size.unwrap_or(DEFAULT_SIZE);
            Bounds::centered(None, gpui::size(px(width), px(height)), cx)
        }
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("webgpu-nodes".into()),
            ..Default::default()
        }),
        // Playback is driven by this window's frames, which must not slow down while another window,
        // such as an output on a second display, has the focus.
        inactive_frame_interval: None,
        ..Default::default()
    };
    // The root hosts the window's dialogs, which the shell opens.
    cx.open_window(options, |window, cx| {
        let shell = cx.new(|cx| Shell::start(start, zoom, window, cx));
        cx.new(|cx| Root::new(gpui::AnyView::from(shell), window, cx))
    })
}

fn live(handle: AnyWindowHandle, cx: &App) -> bool {
    cx.windows().contains(&handle)
}

/// Shows the preferences window, opening it if it isn't open. There is only one.
pub fn open_preferences(cx: &mut App) {
    if let Some(handle) = live_preferences(cx) {
        activate(handle, cx);
        return;
    }
    let bounds = Bounds::centered(None, size(px(560.0), px(480.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Preferences".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let dialog = cx.new(|cx| PreferencesDialog::new(window, cx));
        cx.new(|cx| Root::new(gpui::AnyView::from(dialog), window, cx))
    }) {
        Ok(handle) => cx.global_mut::<Registry>().preferences = Some(handle.into()),
        Err(error) => log::error!("failed to open the preferences window: {error:#}"),
    }
}

/// The preferences window, if it is open.
fn live_preferences(cx: &mut App) -> Option<AnyWindowHandle> {
    let handle = cx.global::<Registry>().preferences?;
    if live(handle, cx) {
        Some(handle)
    } else {
        cx.global_mut::<Registry>().preferences = None;
        None
    }
}

/// Registers an output window opened for the project window `owner`.
pub fn add_output(handle: AnyWindowHandle, owner: EntityId, cx: &mut App) {
    let registry = cx.global_mut::<Registry>();
    registry.outputs.push(OutputWindow { handle, owner });
}

/// The output windows of a project window.
#[cfg(test)]
pub fn outputs_of(owner: EntityId, cx: &App) -> Vec<AnyWindowHandle> {
    cx.try_global::<Registry>()
        .map(|r| {
            r.outputs
                .iter()
                .filter(|o| o.owner == owner)
                .map(|o| o.handle)
                .filter(|h| live(*h, cx))
                .collect()
        })
        .unwrap_or_default()
}

/// Closes the outputs of a project window that is gone.
pub fn owner_closed(owner: EntityId, cx: &mut App) {
    let Some(registry) = cx.try_global::<Registry>() else {
        return;
    };
    let gone: Vec<AnyWindowHandle> = registry
        .outputs
        .iter()
        .filter(|o| o.owner == owner)
        .map(|o| o.handle)
        .collect();
    for handle in gone {
        cx.update_window(handle, |_, window, _| window.remove_window())
            .ok();
    }
    cx.global_mut::<Registry>()
        .outputs
        .retain(|o| o.owner != owner);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::output::OutputView;
    use crate::preferences;
    use crate::shell::{self, NewProject, OpenPreferences, TogglePlayback};

    /// A project window that starts without a project file.
    fn project_window(cx: &mut TestAppContext) -> (Entity<Shell>, VisualTestContext) {
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::start(Start::Blank, None, window, cx));
        cx.run_until_parked();
        (shell, cx.clone())
    }

    fn app(cx: &mut TestAppContext) {
        // Shader compiles finish on a real thread, which wakes the window from outside the scheduler.
        cx.executor().allow_parking();
        cx.update(|cx| {
            theme::init(cx);
            preferences::init(cx);
            shell::init(cx);
            init(cx);
        });
    }

    fn make_dirty(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
        shell.update(cx, |shell, cx| {
            shell.editor().update(cx, |editor, cx| {
                let mut settings = editor.settings();
                settings.canvas.width += 1;
                editor.update_settings(settings, cx);
            });
        });
    }

    fn dirty(shell: &Entity<Shell>, cx: &VisualTestContext) -> bool {
        shell.read_with(cx, |s, cx| s.editor().read(cx).has_unsaved_changes())
    }

    fn playing(shell: &Entity<Shell>, cx: &VisualTestContext) -> bool {
        shell.read_with(cx, |s, cx| s.playback().read(cx).playing())
    }

    /// Closes the window as its close button does: it goes unless the shell asks first.
    fn close(cx: &mut VisualTestContext) -> bool {
        let closes = cx.simulate_close();
        if closes {
            cx.update(|window, _| window.remove_window());
        }
        closes
    }

    /// The windows open once what a closed window releases has been closed in turn.
    fn window_count(cx: &mut TestAppContext) -> usize {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|_| {});
        }
        cx.update(|cx| cx.windows().len())
    }

    #[gpui::test]
    fn project_windows_keep_their_own_state(cx: &mut TestAppContext) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (b, mut cb) = project_window(cx);
        assert_ne!(
            a.read_with(&ca, |s, _| s.playback().entity_id()),
            b.read_with(&cb, |s, _| s.playback().entity_id())
        );

        make_dirty(&a, &mut ca);
        assert!(dirty(&a, &ca));
        assert!(
            !dirty(&b, &cb),
            "editing one project leaves the other alone"
        );

        ca.dispatch_action(TogglePlayback);
        assert!(playing(&a, &ca));
        assert!(!playing(&b, &cb), "playback is per window");
        cb.dispatch_action(TogglePlayback);
        cb.dispatch_action(TogglePlayback);
        assert!(
            playing(&a, &ca) && !playing(&b, &cb),
            "a menu action goes to the window it is dispatched in"
        );
        assert_eq!(window_count(cx), 2);
    }

    #[gpui::test]
    fn closing_asks_only_for_the_window_with_unsaved_changes(cx: &mut TestAppContext) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (_, mut cb) = project_window(cx);
        make_dirty(&a, &mut ca);

        assert!(close(&mut cb), "a saved project closes without asking");
        assert!(!cb.has_pending_prompt());
        assert!(!close(&mut ca));
        assert!(ca.has_pending_prompt());
        ca.simulate_prompt_answer("Cancel");
        ca.run_until_parked();
        assert_eq!(window_count(cx), 1);
    }

    #[gpui::test]
    fn quitting_asks_each_window_with_unsaved_changes(cx: &mut TestAppContext) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (b, mut cb) = project_window(cx);
        let (_, cc) = project_window(cx);
        make_dirty(&a, &mut ca);
        make_dirty(&b, &mut cb);
        let quit = Rc::new(Cell::new(false));
        let done = quit.clone();
        cx.update(|cx| shell::confirm_all_discards(cx, move |_| done.set(true)));

        for _ in 0..2 {
            let ask = if ca.has_pending_prompt() {
                &mut ca
            } else {
                &mut cb
            };
            assert!(ask.has_pending_prompt(), "each dirty window asks in turn");
            assert!(!quit.get());
            ask.simulate_prompt_answer("Discard");
            ask.run_until_parked();
        }
        assert!(!cc.has_pending_prompt(), "a saved project isn't asked");
        assert!(quit.get(), "quitting goes on once every window has agreed");
    }

    #[gpui::test]
    fn one_window_refusing_stops_the_quit(cx: &mut TestAppContext) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (b, mut cb) = project_window(cx);
        make_dirty(&a, &mut ca);
        make_dirty(&b, &mut cb);
        let quit = Rc::new(Cell::new(false));
        let done = quit.clone();
        cx.update(|cx| shell::confirm_all_discards(cx, move |_| done.set(true)));
        let ask = if ca.has_pending_prompt() {
            &mut ca
        } else {
            &mut cb
        };
        ask.simulate_prompt_answer("Cancel");
        ask.run_until_parked();
        assert!(!quit.get());
        assert!(
            !ca.has_pending_prompt() && !cb.has_pending_prompt(),
            "nothing else is asked"
        );
    }

    #[gpui::test]
    fn new_replaces_nothing_in_an_untouched_window_and_opens_a_window_otherwise(
        cx: &mut TestAppContext,
    ) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        ca.dispatch_action(NewProject);
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            1,
            "a new project is already what New would open"
        );

        make_dirty(&a, &mut ca);
        ca.dispatch_action(NewProject);
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            2,
            "New opens a window beside a project with changes"
        );
        assert!(dirty(&a, &ca), "and keeps them");
    }

    #[gpui::test]
    fn there_is_one_preferences_window_for_every_project(cx: &mut TestAppContext) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (_, mut cb) = project_window(cx);
        let preferences_open =
            |cx: &mut TestAppContext| cx.update(|cx| cx.global::<Registry>().preferences.is_some());

        ca.dispatch_action(OpenPreferences);
        cx.run_until_parked();
        assert_eq!(window_count(cx), 3);
        assert!(preferences_open(cx));

        cb.dispatch_action(OpenPreferences);
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            3,
            "opening it again shows the one that is open"
        );

        assert!(close(&mut cb));
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            2,
            "the preferences don't close with a project"
        );
        assert!(preferences_open(cx));
        drop(a);
        assert!(close(&mut ca));
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            1,
            "nor do they follow the project window that opened them"
        );
        assert!(preferences_open(cx));
    }

    #[gpui::test]
    fn an_output_window_shows_its_projects_playback_without_drawing_frames(
        cx: &mut TestAppContext,
    ) {
        app(cx);
        let (a, mut ca) = project_window(cx);
        let (b, mut cb) = project_window(cx);
        ca.dispatch_action(NewOutputWindow);
        cx.run_until_parked();
        assert_eq!(window_count(cx), 3);
        let outputs = cx.update(|cx| outputs_of(a.entity_id(), cx));
        assert_eq!(outputs.len(), 1);
        assert!(cx.update(|cx| outputs_of(b.entity_id(), cx)).is_empty());

        let view = cx
            .update(|cx| {
                outputs[0]
                    .downcast::<OutputView>()
                    .and_then(|h| h.entity(cx).ok())
            })
            .expect("an output view");
        assert_eq!(
            view.read_with(&ca, |v, _| v.playback().entity_id()),
            a.read_with(&ca, |s, _| s.playback().entity_id()),
            "it shows the playback of the window it was opened from"
        );

        let submitted = a.read_with(&ca, |s, cx| s.playback().read(cx).submitted());
        let mut output = VisualTestContext::from_window(outputs[0], cx);
        for _ in 0..5 {
            output.update(|window, _| window.refresh());
            output.run_until_parked();
        }
        assert_eq!(
            a.read_with(&ca, |s, cx| s.playback().read(cx).submitted()),
            submitted,
            "drawing the output renders nothing"
        );

        view.update(&mut output, |v, cx| v.toggle_stretch(cx));
        assert!(view.read_with(&output, |v, _| v.stretch()));

        assert!(close(&mut cb));
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            2,
            "an output stays when another project closes"
        );
        drop(a);
        assert!(close(&mut ca));
        cx.run_until_parked();
        assert_eq!(
            window_count(cx),
            0,
            "the outputs close with their project window"
        );
    }
}
