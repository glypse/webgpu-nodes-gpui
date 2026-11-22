//! An output window: nothing but the frame its project's playback last drew, for showing the
//! visuals on another display.

use std::sync::Arc;

use anyhow::Result;
use gpui::{
    AnyWindowHandle, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, RenderImage, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
    canvas, div, img, px, size,
};
use guard::SafeListener as _;

use crate::playback::{Playback, Shown, device_size};
use crate::settings::Page;
use crate::shell::{OpenPreferences, OpenProjectSettings, OpenSettings, Shell};
use crate::windows::{self, NewOutputWindow, ToggleOutputStretch};

const DEFAULT_SIZE: (f32, f32) = (960.0, 540.0);

/// Opens an output window showing `playback`'s frames. It reports its size to the playback as a
/// preview area does, and is followed while it is the one last used.
pub fn open(shell: &Entity<Shell>, playback: &Entity<Playback>, owner: AnyWindowHandle, cx: &mut App) -> Result<AnyWindowHandle> {
    let bounds = gpui::Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Output".into()), ..Default::default() }),
        // On a second display the window is rarely the active one, and must keep its frame rate.
        inactive_frame_interval: None,
        ..Default::default()
    };
    let (shell_weak, playback) = (shell.downgrade(), playback.clone());
    let handle = cx.open_window(options, move |window, cx| {
        cx.new(|cx| OutputView::new(shell_weak, playback, owner, window, cx))
    })?;
    let handle: AnyWindowHandle = handle.into();
    windows::add_output(handle, shell.entity_id(), cx);
    Ok(handle)
}

pub struct OutputView {
    shell: WeakEntity<Shell>,
    playback: Entity<Playback>,
    /// The project window, which draws the frames: the playback is scheduled from its window.
    owner: AnyWindowHandle,
    /// Stretches the frame over the window instead of fitting it with black bars.
    stretch: bool,
    image: Option<Arc<RenderImage>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl OutputView {
    fn new(
        shell: WeakEntity<Shell>,
        playback: Entity<Playback>,
        owner: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            cx.observe(&playback, |_, _, cx| cx.notify()),
            // The window last used is the one frames follow while the canvas follows a preview.
            cx.observe_window_activation(window, move |this, window, cx| {
                if window.is_window_active() {
                    let playback = this.playback.clone();
                    cx.update_window(this.owner, |_, owner, cx| {
                        playback.update(cx, |p, cx| p.follow_pane(id, owner, cx));
                    })
                    .ok();
                }
            }),
            cx.on_release({
                let playback = playback.clone();
                move |_, cx| {
                    playback.update(cx, |p, cx| p.forget_pane(id, cx));
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { shell, playback, owner, stretch: false, image: None, focus, _subscriptions: subscriptions }
    }

    #[cfg(test)]
    pub fn playback(&self) -> &Entity<Playback> {
        &self.playback
    }

    #[cfg(test)]
    pub fn stretch(&self) -> bool {
        self.stretch
    }

    pub fn toggle_stretch(&mut self, cx: &mut Context<Self>) {
        self.stretch = !self.stretch;
        cx.notify();
    }

    fn show_settings(&self, page: Page, cx: &mut App) {
        if let Some(shell) = self.shell.upgrade() {
            shell.update(cx, |shell, cx| shell.show_settings(page, cx));
        }
    }
}

impl Focusable for OutputView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OutputView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("OutputView", || {
            let shown = self.playback.read(cx).shown();
            // The window's image cache holds each frame drawn as an image until it is dropped.
            let image = match &shown {
                Some(Shown::Image(image)) => Some(image.clone()),
                _ => None,
            };
            if let Some(old) = std::mem::replace(&mut self.image, image.clone())
                && image.is_none_or(|current| !Arc::ptr_eq(&current, &old))
                && let Err(error) = window.drop_image(old)
            {
                log::warn!("dropping the previous frame failed: {error:#}");
            }
            let fit = if self.stretch { ObjectFit::Fill } else { ObjectFit::Contain };
            let id = cx.entity_id();
            let playback = self.playback.clone();
            let owner = self.owner;
            div()
                .id("output")
                .debug_selector(|| "output".into())
                .key_context("Output")
                .track_focus(&self.focus)
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(gpui::black())
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            guard::guard("output layout", || {
                                let scale = window.scale_factor();
                                let Some(pane) =
                                    device_size(f32::from(bounds.size.width), f32::from(bounds.size.height), scale)
                                else {
                                    return;
                                };
                                // The size is registered through the project window, so that the frames it
                                // changes are scheduled from there and not from a window that may close.
                                cx.update_window(owner, |_, owner, cx| {
                                    playback.update(cx, |p, cx| p.set_pane(id, pane, owner, cx));
                                })
                                .ok();
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .children(shown.map(|shown| match shown {
                    #[cfg(target_os = "macos")]
                    Shown::Surface(frame) => gpui::surface(frame).object_fit(fit).absolute().size_full().into_any_element(),
                    Shown::Image(image) => img(image).object_fit(fit).absolute().size_full().into_any_element(),
                }))
                .on_click(cx.safe_listener(|_, event: &ClickEvent, window, _| {
                    if event.click_count() == 2 {
                        window.toggle_fullscreen();
                    }
                }))
                .on_action(cx.safe_listener(|this, _: &ToggleOutputStretch, _, cx| this.toggle_stretch(cx)))
                .on_action(cx.safe_listener(move |this, _: &NewOutputWindow, _, cx| {
                    if let Some(shell) = this.shell.upgrade() {
                        shell.update(cx, |shell, cx| shell.new_output(owner, cx));
                    }
                }))
                .on_action(cx.safe_listener(|_, _: &windows::CloseWindow, window, _| window.remove_window()))
                .on_action(cx.safe_listener(|this, _: &OpenSettings, _, cx| this.show_settings(Page::Canvas, cx)))
                .on_action(cx.safe_listener(|this, _: &OpenProjectSettings, _, cx| this.show_settings(Page::Canvas, cx)))
                .on_action(cx.safe_listener(|this, _: &OpenPreferences, _, cx| this.show_settings(Page::Appearance, cx)))
        })
    }
}
