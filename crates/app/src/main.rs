mod assets;
mod cli;
mod code_view;
mod editor;
mod export_dialog;
mod graph;
mod layout;
mod playback;
mod preferences;
mod preview;
mod probe;
mod project;
#[cfg(test)]
mod scenes;
mod settings;
mod shell;
mod timeline;

use gpui::{
    App, AppContext as _, Bounds, Global, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, actions, px, size,
};

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser as _;
use gpui::{AsyncApp, WindowHandle};
use preferences::ToggleDeveloperTools;
use shell::{
    ExportFrame, ExportVideo, ImportMedia, MenuActualSizePreview, MenuFitPreview, MenuZoomPreviewIn,
    MenuZoomPreviewOut, OpenPreferences, OpenProject, OpenProjectSettings, OpenSettings, SaveProject, Shell,
    ToggleMode, TogglePlayback,
};

actions!(app, [Quit]);

/// Whether developer tools are on and playback runs, as the menus show them.
#[derive(Default)]
struct MenuState {
    playing: bool,
}

impl Global for MenuState {}

/// Sets the menus from the preferences and playback. gpui reads a menu's labels and checks only when
/// they are set, so this runs again whenever either changes.
pub(crate) fn refresh_menus(cx: &mut App) {
    let developer_tools = preferences::developer_tools_on(cx);
    let playing = cx.try_global::<MenuState>().is_some_and(|state| state.playing);
    cx.set_menus(menus(developer_tools, playing));
}

/// Records whether playback runs, for the Play and Pause item.
pub(crate) fn set_playing(cx: &mut App, playing: bool) {
    if cx.try_global::<MenuState>().is_some_and(|state| state.playing == playing) {
        return;
    }
    cx.set_global(MenuState { playing });
    refresh_menus(cx);
}

/// The File, Edit and Display menus of the original, in its order. An item whose action the focused
/// element doesn't handle is greyed out; the original's items with no action yet are left out.
fn menus(developer_tools: bool, playing: bool) -> Vec<Menu> {
    let play_or_pause = if playing { "Pause" } else { "Play" };
    vec![
        Menu {
            name: "webgpu-nodes".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Settings...", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Save", SaveProject),
                MenuItem::action("Upload", OpenProject),
                MenuItem::action("Import media…", ImportMedia),
                MenuItem::separator(),
                MenuItem::action("Export frame as PNG", ExportFrame),
                MenuItem::action("Export video…", ExportVideo),
                MenuItem::separator(),
                MenuItem::action("Project settings…", OpenProjectSettings),
                MenuItem::separator(),
                MenuItem::action("Developer tools", ToggleDeveloperTools).checked(developer_tools),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Undo", editor::Undo),
                MenuItem::action("Redo", editor::Redo),
                MenuItem::separator(),
                MenuItem::action("Group selected", editor::GroupSelected),
                MenuItem::action("Ungroup selected", editor::UngroupSelected),
                MenuItem::separator(),
                MenuItem::action("Preview selected node", editor::PreviewSelectedNode),
                MenuItem::separator(),
                MenuItem::action("Layout selected nodes", editor::LayoutSelected),
                MenuItem::action("Layout graph", editor::LayoutGraph),
                MenuItem::action("Layout document", editor::LayoutDocument),
                MenuItem::separator(),
                MenuItem::action("Remove all unused group references", editor::PruneUnusedGroups),
                MenuItem::separator(),
                MenuItem::action("Enter node group", editor::EnterNodeGroup),
                MenuItem::action("Exit node group", editor::ExitNodeGroup),
                MenuItem::separator(),
                MenuItem::action("Toggle node sheet", editor::ToggleNodeSheet),
                MenuItem::separator(),
                MenuItem::action("Preferences…", OpenPreferences),
            ],
        },
        Menu {
            name: "Display".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Toggle mode", ToggleMode),
                MenuItem::separator(),
                MenuItem::action(play_or_pause, TogglePlayback),
                MenuItem::separator(),
                MenuItem::action("Zoom preview in", MenuZoomPreviewIn),
                MenuItem::action("Zoom preview out", MenuZoomPreviewOut),
                MenuItem::action("Fit preview", MenuFitPreview),
                MenuItem::action("Preview at actual size", MenuActualSizePreview),
            ],
        },
    ]
}

/// Set when `--screenshot` could not be written, so the process exits non-zero.
static SCREENSHOT_FAILED: AtomicBool = AtomicBool::new(false);

/// How long a bounded run may go without presenting a frame before it quits anyway.
const STALL: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    guard::install_panic_hook();
    if cli.records()
        && let Err(error) = probe::install(cli.profile.as_deref())
    {
        log::error!("{error:#}");
        return ExitCode::FAILURE;
    }
    let launch = cli.clone();
    gpui_platform::application().with_assets(theme::Assets).run(move |cx| {
        theme::init(cx);
        preferences::init(cx);
        shell::init(cx);
        editor::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| shell::request_quit(cx));
        refresh_menus(cx);
        let window_size = launch.window.unwrap_or(cli::Size { width: 1400.0, height: 860.0 });
        let bounds = Bounds::centered(None, size(px(window_size.width), px(window_size.height)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("webgpu-nodes".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let project = launch.project.clone();
        // The root hosts the window's dialogs, which the shell opens.
        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| Shell::new(project, launch.zoom, window, cx));
            cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(shell), window, cx))
        });
        match opened {
            Ok(handle) => {
                if launch.play
                    && let Err(error) = handle.update(cx, |root, window, cx| {
                        if let Ok(shell) = root.view().clone().downcast::<Shell>() {
                            shell.update(cx, |shell, cx| shell.play(window, cx));
                        }
                    })
                {
                    log::error!("failed to start playback: {error:#}");
                }
                if launch.bounded() || launch.screenshot.is_some() {
                    let launch = launch.clone();
                    cx.spawn(async move |cx| wind_down(launch, handle, cx).await).detach();
                }
            }
            Err(e) => {
                log::error!("failed to open the window: {e:#}");
                cx.quit();
            }
        }
        cx.on_app_quit(|_| {
            probe::finish();
            std::future::ready(())
        })
        .detach();
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
    if SCREENSHOT_FAILED.load(Ordering::Relaxed) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Quits once the `--seconds` or `--frames` bound is reached, after taking the
/// `--screenshot` if asked for.
async fn wind_down(launch: cli::Cli, handle: WindowHandle<gpui_component::Root>, cx: &mut AsyncApp) {
    if launch.bounded() {
        let started = Instant::now();
        let mut frames = probe::frames();
        let mut last_change = Instant::now();
        loop {
            cx.background_executor().timer(Duration::from_millis(10)).await;
            let now = probe::frames();
            if now != frames {
                frames = now;
                last_change = Instant::now();
            }
            if launch.seconds.is_some_and(|s| started.elapsed().as_secs_f64() >= s) {
                break;
            }
            if launch.frames.is_some_and(|n| frames >= n) {
                break;
            }
            if launch.frames.is_some() && last_change.elapsed() > STALL {
                log::warn!("no frame presented for {STALL:?}; quitting");
                break;
            }
        }
    }
    if let Some(path) = &launch.screenshot {
        // Draw a few frames more, so the window shows the state the run ended in.
        for _ in 0..3 {
            if cx.update_window(handle.into(), |_, window, _| window.refresh()).is_err() {
                break;
            }
            cx.background_executor().timer(Duration::from_millis(100)).await;
        }
        if let Err(error) = probe::save_window(path) {
            log::error!("screenshot failed: {error:#}");
            SCREENSHOT_FAILED.store(true, Ordering::Relaxed);
        }
    }
    cx.update(|cx| cx.quit());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The item labels of a menu, separators as "-".
    fn labels(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                MenuItem::Action { name, .. } => name.to_string(),
                MenuItem::Separator => "-".to_owned(),
                _ => "?".to_owned(),
            })
            .collect()
    }

    fn menu<'a>(menus: &'a [Menu], name: &str) -> &'a Menu {
        menus.iter().find(|m| m.name.as_ref() == name).expect("the menu exists")
    }

    #[test]
    fn file_edit_and_display_menus_list_the_original_items_in_order() {
        let menus = menus(false, false);
        assert_eq!(
            labels(menu(&menus, "File")),
            [
                "Save", "Upload", "Import media…", "-", "Export frame as PNG", "Export video…", "-",
                "Project settings…", "-", "Developer tools",
            ]
        );
        assert_eq!(
            labels(menu(&menus, "Edit")),
            [
                "Undo", "Redo", "-", "Group selected", "Ungroup selected", "-", "Preview selected node", "-",
                "Layout selected nodes", "Layout graph", "Layout document", "-",
                "Remove all unused group references", "-", "Enter node group", "Exit node group", "-",
                "Toggle node sheet", "-", "Preferences…",
            ]
        );
        assert_eq!(
            labels(menu(&menus, "Display")),
            [
                "Toggle mode", "-", "Play", "-", "Zoom preview in", "Zoom preview out", "Fit preview",
                "Preview at actual size",
            ]
        );
    }

    #[test]
    fn developer_tools_is_checked_when_on_and_play_reads_pause_while_playing() {
        let menus = menus(true, true);
        let file = menu(&menus, "File");
        assert!(matches!(file.items.last(), Some(item) if item.is_checked()));
        assert_eq!(labels(menu(&menus, "Display"))[2], "Pause");
    }
}
