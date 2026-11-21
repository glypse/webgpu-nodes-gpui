mod code_view;
mod editor;
mod graph;
mod layout;
mod playback;
mod preferences;
mod preview;
mod project;
mod settings;
mod shell;
mod timeline;

use gpui::{
    AppContext as _, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, actions, px, size,
};

use shell::{NewProject, OpenProject, OpenSettings, Redo, SaveProject, SaveProjectAs, Shell, Undo};

actions!(app, [Quit]);

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "webgpu-nodes".into(),
            items: vec![
                MenuItem::action("Settings...", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New", NewProject),
                MenuItem::action("Open...", OpenProject),
                MenuItem::separator(),
                MenuItem::action("Save", SaveProject),
                MenuItem::action("Save As...", SaveProjectAs),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![MenuItem::action("Undo", Undo), MenuItem::action("Redo", Redo)],
        },
    ]
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    guard::install_panic_hook();
    Application::new().with_assets(theme::Assets).run(|cx| {
        theme::init(cx);
        preferences::init(cx);
        shell::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(menus());
        let bounds = Bounds::centered(None, size(px(1400.0), px(860.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("webgpu-nodes".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| cx.new(|cx| Shell::new(window, cx)));
        if let Err(e) = opened {
            log::error!("failed to open the window: {e:#}");
            cx.quit();
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
