mod editor;
mod renderer;

use gpui::{
    AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

use editor::Editor;

fn main() {
    Application::new().run(|cx| {
        let bounds = Bounds::centered(None, size(px(1400.0), px(860.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("webgpu-nodes".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| Editor::new(window, cx).expect("the preview needs a GPU"))
        });
        if let Err(e) = opened {
            eprintln!("failed to open the window: {e}");
            cx.quit();
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
