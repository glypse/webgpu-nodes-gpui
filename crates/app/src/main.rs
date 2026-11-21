mod assets;
mod cli;
mod code_view;
mod editor;
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
    AppContext as _, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, actions, px, size,
};

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser as _;
use gpui::{AsyncApp, WindowHandle};
use shell::{ImportMedia, NewProject, OpenProject, OpenSettings, Redo, SaveProject, SaveProjectAs, Shell, Undo};

actions!(app, [Quit]);

fn menus() -> Vec<Menu> {
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
                MenuItem::action("New", NewProject),
                MenuItem::action("Open...", OpenProject),
                MenuItem::separator(),
                MenuItem::action("Save", SaveProject),
                MenuItem::action("Save As...", SaveProjectAs),
                MenuItem::separator(),
                MenuItem::action("Import media...", ImportMedia),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![MenuItem::action("Undo", Undo), MenuItem::action("Redo", Redo)],
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
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| shell::request_quit(cx));
        cx.set_menus(menus());
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
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| Shell::new(project, launch.zoom, window, cx))
        });
        match opened {
            Ok(handle) => {
                if launch.play
                    && let Err(error) = handle.update(cx, |shell, window, cx| shell.play(window, cx))
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
async fn wind_down(launch: cli::Cli, handle: WindowHandle<Shell>, cx: &mut AsyncApp) {
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
