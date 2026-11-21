mod code_view;
mod editor;
mod graph;
mod playback;
mod preview;
mod shell;
mod timeline;

use gpui::{
    AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

use shell::Shell;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    guard::install_panic_hook();
    Application::new().with_assets(theme::Assets).run(|cx| {
        theme::init(cx);
        shell::init(cx);
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
