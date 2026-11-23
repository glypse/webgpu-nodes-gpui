mod assets;
mod cli;
mod code_view;
mod editor;
mod graph;
mod group_bundles;
mod layout;
mod output;
mod perf;
mod playback;
mod preferences;
mod preferences_window;
mod preview;
mod probe;
mod project;
mod properties;
#[cfg(test)]
mod scenes;
mod shell;
mod timeline;
mod video_export;
mod windows;

use gpui::{App, AppContext as _, Global, KeyBinding, Menu, MenuItem, actions};

use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::Parser as _;
use gpui::{AsyncApp, WindowHandle, WindowingRequest};
use preferences::ToggleDeveloperTools;
use shader_graph::graph::{GraphModel, GraphNode, GraphProject};
use shader_graph::layout::LayoutOptions;
use shell::{
    ExportFrame, ExportVideo, ImportGroups, ImportMedia, MenuActualSizePreview, MenuFitPreview,
    MenuZoomPreviewIn, MenuZoomPreviewOut, NewProject, OpenPreferences, OpenProject,
    OpenProjectSettings, SaveProject, SaveProjectAs, Shell, Start, ToggleMode, TogglePlayback,
};
use windows::{CloseWindow, NewOutputWindow, ToggleFullscreen, ToggleOutputStretch};

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
    let playing = cx
        .try_global::<MenuState>()
        .is_some_and(|state| state.playing);
    cx.set_menus(menus(developer_tools, playing));
}

/// Records whether playback runs, for the Play and Pause item.
pub(crate) fn set_playing(cx: &mut App, playing: bool) {
    if cx
        .try_global::<MenuState>()
        .is_some_and(|state| state.playing == playing)
    {
        return;
    }
    cx.set_global(MenuState { playing });
    refresh_menus(cx);
}

/// The File, Edit and Display menus, in order. An item whose action the focused element doesn't
/// handle is greyed out; items with no action yet are left out.
fn menus(developer_tools: bool, playing: bool) -> Vec<Menu> {
    let play_or_pause = if playing { "Pause" } else { "Play" };
    vec![
        Menu {
            name: "webgpu-nodes".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Preferences…", OpenPreferences),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New", NewProject),
                MenuItem::action("Save", SaveProject),
                MenuItem::action("Save as…", SaveProjectAs),
                MenuItem::action("Open…", OpenProject),
                MenuItem::action("Import media…", ImportMedia),
                MenuItem::separator(),
                MenuItem::action("Export selected group", editor::ExportSelectedGroup),
                MenuItem::action("Import groups", ImportGroups),
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
                MenuItem::action(
                    "Remove all unused group references",
                    editor::PruneUnusedGroups,
                ),
                MenuItem::separator(),
                MenuItem::action("Enter node group", editor::EnterNodeGroup),
                MenuItem::action("Exit node group", editor::ExitNodeGroup),
                MenuItem::separator(),
                MenuItem::action("Toggle node sheet", editor::ToggleNodeSheet),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New output window", NewOutputWindow),
                MenuItem::separator(),
                MenuItem::action("Toggle full screen", ToggleFullscreen),
                MenuItem::action("Toggle output stretch", ToggleOutputStretch),
                MenuItem::separator(),
                MenuItem::action("Close window", CloseWindow),
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
    match cli.layout_project.as_deref() {
        Some([input, output]) => return layout_project_headless(input, output),
        Some(_) => {
            eprintln!("webgpu-nodes: --layout-project takes an input and an output path");
            return ExitCode::FAILURE;
        }
        None => {}
    }
    if cli.records()
        && let Err(error) = probe::install(cli.profile.as_deref())
    {
        log::error!("{error:#}");
        return ExitCode::FAILURE;
    }
    let launch = cli.clone();
    gpui_platform::application()
        .with_assets(theme::Assets)
        .run(move |cx| {
            theme::init(cx);
            preferences::init(cx);
            shell::init(cx);
            windows::init(cx);
            editor::init(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            // Global listeners run while the focused window is updated, so quitting waits until it is done.
            cx.on_action(|_: &Quit, cx| cx.defer(shell::request_quit));
            refresh_menus(cx);
            let window_size = launch
                .window
                .map_or(windows::DEFAULT_SIZE, |size| (size.width, size.height));
            let opened = windows::open_project(
                Start::Launch(launch.project.clone()),
                launch.zoom,
                None,
                Some(window_size),
                cx,
            );
            match opened {
                Ok(handle) => {
                    if launch.play
                        && let Err(error) = handle.update(cx, |root, window, cx| {
                            guard::guard("start playback", || {
                                if let Ok(shell) = root.view().clone().downcast::<Shell>() {
                                    shell.update(cx, |shell, cx| shell.play(window, cx));
                                }
                            });
                        })
                    {
                        log::error!("failed to start playback: {error:#}");
                    }
                    if launch.output
                        && let Err(error) = handle.update(cx, |root, window, cx| {
                            guard::guard("open the output window", || {
                                if let Ok(shell) = root.view().clone().downcast::<Shell>() {
                                    let owner = window.window_handle();
                                    shell.update(cx, |shell, cx| shell.new_output(owner, cx));
                                }
                            });
                        })
                    {
                        log::error!("failed to open the output window: {error:#}");
                    }
                    if launch.bounded() || launch.screenshot.is_some() {
                        let launch = launch.clone();
                        cx.spawn(async move |cx| wind_down(launch, handle, cx).await)
                            .detach();
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
            cx.on_window_closed(|cx, _| {
                if windows::project_windows(cx).is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    if SCREENSHOT_FAILED.load(Ordering::Relaxed) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `--layout-project`: lays out the bare-JSON project at `input` with the node
/// sizes the editor draws and writes it to `output`. No window is shown.
fn layout_project_headless(input: &Path, output: &Path) -> ExitCode {
    let project = match read_bare_project(input) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("webgpu-nodes: {}: {error:#}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let (input_name, output) = (input.display().to_string(), output.to_path_buf());
    gpui_platform::application()
        .with_assets(theme::Assets)
        .with_windowing(WindowingRequest::Headless)
        .run(move |cx| {
            theme::init(cx);
            preferences::init(cx);
            let status = match layout_in_app(project, &output, cx) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("webgpu-nodes: laying out {input_name}: {error:#}");
                    1
                }
            };
            // Quitting goes through NSApplication's terminate, which exits with status 0.
            std::process::exit(status);
        });
    eprintln!("webgpu-nodes: the layout did not finish");
    ExitCode::FAILURE
}

fn read_bare_project(path: &Path) -> anyhow::Result<GraphProject> {
    let bytes = project::read_capped(path).context("reading the file")?;
    let text = std::str::from_utf8(&bytes).context("it isn't bare JSON")?;
    Ok(GraphProject::from_json(text)?)
}

fn layout_in_app(project: GraphProject, output: &Path, cx: &mut App) -> anyhow::Result<()> {
    let mut model = GraphModel::new(project);
    let definitions = model.layout_definitions();
    // The app quits straight after, so the measuring window is left to it.
    let measured = editor::measure_documents(model.project(), &definitions, cx)
        .context("measuring the nodes")?
        .geometry;
    let measure = |doc: &str, node: &GraphNode| measured.get(doc)?.get(&node.id).cloned();
    let options = LayoutOptions {
        measure: Some(&measure),
        ..Default::default()
    };
    model
        .layout_project(&options)
        .context("arranging the nodes")?;
    fs::write(output, model.project().to_json())
        .with_context(|| format!("writing {}", output.display()))
}

/// Quits once the `--seconds` or `--frames` bound is reached, after taking the
/// `--screenshot` if asked for.
async fn wind_down(
    launch: cli::Cli,
    handle: WindowHandle<gpui_component::Root>,
    cx: &mut AsyncApp,
) {
    if launch.bounded() {
        let started = Instant::now();
        let mut frames = probe::frames();
        let mut last_change = Instant::now();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
            let now = probe::frames();
            if now != frames {
                frames = now;
                last_change = Instant::now();
            }
            if launch
                .seconds
                .is_some_and(|s| started.elapsed().as_secs_f64() >= s)
            {
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
            if cx
                .update_window(handle.into(), |_, window, _| window.refresh())
                .is_err()
            {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
        if let Err(error) = probe::save_window(path, launch.output.then_some("Output")) {
            log::error!("screenshot failed: {error:#}");
            SCREENSHOT_FAILED.store(true, Ordering::Relaxed);
        }
    }
    cx.update(|cx| cx.quit());
}

#[cfg(test)]
mod tests {
    use gpui::Action as _;

    use super::*;

    /// The actions of a menu's items in order, separators as "-".
    fn entries(menu: &Menu) -> Vec<&'static str> {
        menu.items
            .iter()
            .map(|item| match item {
                MenuItem::Action { action, .. } => action.name(),
                MenuItem::Separator => "-",
                _ => "?",
            })
            .collect()
    }

    fn menu<'a>(menus: &'a [Menu], name: &str) -> &'a Menu {
        menus
            .iter()
            .find(|m| m.name.as_ref() == name)
            .expect("the menu exists")
    }

    #[test]
    fn file_edit_and_display_menus_list_actions_in_order() {
        let menus = menus(false, false);
        assert_eq!(
            entries(menu(&menus, "File")),
            [
                NewProject.name(),
                SaveProject.name(),
                SaveProjectAs.name(),
                OpenProject.name(),
                ImportMedia.name(),
                "-",
                editor::ExportSelectedGroup.name(),
                ImportGroups.name(),
                "-",
                ExportFrame.name(),
                ExportVideo.name(),
                "-",
                OpenProjectSettings.name(),
                "-",
                ToggleDeveloperTools.name(),
            ]
        );
        assert_eq!(
            entries(menu(&menus, "Window")),
            [
                NewOutputWindow.name(),
                "-",
                ToggleFullscreen.name(),
                ToggleOutputStretch.name(),
                "-",
                CloseWindow.name(),
            ]
        );
        assert_eq!(
            entries(menu(&menus, "Edit")),
            [
                editor::Undo.name(),
                editor::Redo.name(),
                "-",
                editor::GroupSelected.name(),
                editor::UngroupSelected.name(),
                "-",
                editor::PreviewSelectedNode.name(),
                "-",
                editor::LayoutSelected.name(),
                editor::LayoutGraph.name(),
                editor::LayoutDocument.name(),
                "-",
                editor::PruneUnusedGroups.name(),
                "-",
                editor::EnterNodeGroup.name(),
                editor::ExitNodeGroup.name(),
                "-",
                editor::ToggleNodeSheet.name(),
            ]
        );
        assert_eq!(
            entries(menu(&menus, "Display")),
            [
                ToggleMode.name(),
                "-",
                TogglePlayback.name(),
                "-",
                MenuZoomPreviewIn.name(),
                MenuZoomPreviewOut.name(),
                MenuFitPreview.name(),
                MenuActualSizePreview.name(),
            ]
        );
    }

    #[test]
    fn developer_tools_is_checked_when_on_and_play_reads_pause_while_playing() {
        let menus = menus(true, true);
        let file = menu(&menus, "File");
        assert!(matches!(file.items.last(), Some(item) if item.is_checked()));
        assert!(matches!(
            &menu(&menus, "Display").items[2],
            MenuItem::Action { name, .. } if name.as_ref() == "Pause"
        ));
    }
}
