//! The preferences window: the user's own settings, kept on this computer and
//! shared by every project. Project settings live in the Properties area.

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Disableable as _, Sizable as _};
use guard::SafeListener as _;
use theme::{ActiveTheme as _, IconName};

use crate::preferences::{self, Preferences, ThemePreference};
use crate::properties::{field, segmented, segmented_with_icons};

/// The preferences window's contents.
pub struct PreferencesDialog {
    focus: FocusHandle,
    font: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl PreferencesDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let font_text = cx.global::<Preferences>().font.clone();
        let font = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(theme::INTERFACE_FONT);
            input.set_value(font_text, window, cx);
            input
        });
        // Applied on Enter or when the field loses focus, not on each keystroke, since each one saves the file.
        let subscriptions =
            vec![
                cx.subscribe_in(&font, window, |_, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let text = input.read(cx).value().trim().to_owned();
                        guard::guard("interface font", || {
                            preferences::update(window, cx, |p| p.font = text);
                        });
                    }
                }),
            ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            font,
            _subscriptions: subscriptions,
        }
    }
}

impl Focusable for PreferencesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PreferencesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("preferences", || {
            preferences::apply(window, cx);
            let theme = cx.theme().clone();
            let c = &theme.colors;
            let current = cx.global::<Preferences>().clone();

            let themes = [
                ThemePreference::Light,
                ThemePreference::Dark,
                ThemePreference::System,
            ];
            let selected = themes.iter().position(|&t| t == current.theme).unwrap_or(2);
            let theme_field = field(
                "Theme",
                None,
                segmented_with_icons(
                    "theme",
                    [
                        (IconName::Sun, "Light"),
                        (IconName::Moon, "Dark"),
                        (IconName::Desktop, "System"),
                    ],
                    selected,
                    move |index, window, cx| {
                        if let Some(&choice) = themes.get(index) {
                            guard::guard("theme", || {
                                preferences::update(window, cx, |p| p.theme = choice)
                            });
                        }
                    },
                ),
                cx,
            );

            let font_field = field(
                "Interface font",
                Some("A font family, e.g. “Inter”. Leave empty for the default.".to_owned()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(320.0))
                            .px_1()
                            .border_1()
                            .border_color(c.input)
                            .rounded(theme.radius.md)
                            .child(Input::new(&self.font).appearance(false)),
                    )
                    .child(
                        Button::new("reset-font")
                            .ghost()
                            .small()
                            .label("Reset")
                            .disabled(current.font.is_empty())
                            .on_click(cx.safe_listener(|this, _: &ClickEvent, window, cx| {
                                this.font
                                    .update(cx, |f, cx| f.set_value(String::new(), window, cx));
                                preferences::update(window, cx, |p| p.font.clear());
                            })),
                    ),
                cx,
            );

            let developer_tools = field(
                "Developer tools",
                Some("Lists the developer nodes in the add menu, and logs the shader code each graph compiles to.".to_owned()),
                segmented(
                    "developer-tools",
                    ["On", "Off"],
                    if current.developer_tools_enabled() { 0 } else { 1 },
                    |index, window, cx| {
                        guard::guard("developer tools", || {
                            preferences::update(window, cx, |p| p.developer_tools = Some(index == 0));
                        });
                        cx.refresh_windows();
                    },
                ),
                cx,
            );

            div()
                .id("preferences-card")
                .debug_selector(|| "preferences-card".into())
                .track_focus(&self.focus)
                .key_context("Preferences")
                .flex()
                .flex_col()
                .size_full()
                .gap_6()
                .p_4()
                .overflow_y_scroll()
                .bg(c.card)
                .text_color(c.card_foreground)
                .text_size(theme.text.sm)
                .font_family(theme.font.clone())
                .on_key_down(cx.safe_listener(|_, event: &KeyDownEvent, window, _| {
                    if event.keystroke.key == "escape" {
                        window.remove_window();
                    }
                }))
                .child(
                    div()
                        .text_size(theme.text.base)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Preferences"),
                )
                .child(theme_field)
                .child(font_field)
                .child(developer_tools)
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(theme::init);
        cx.update(preferences::init);
        let (_, cx) = cx.add_window_view(PreferencesDialog::new);
        cx.clone()
    }

    #[gpui::test]
    fn the_preferences_window_shows_preferences_and_no_project_settings(cx: &mut TestAppContext) {
        let mut cx = window(cx);
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("preferences-card").is_some(),
            "the window draws its card"
        );
        assert!(
            cx.debug_bounds("properties-panel").is_none(),
            "project settings are in the Properties area, not here"
        );
    }

    #[gpui::test]
    fn the_theme_and_developer_tools_change_the_preferences(cx: &mut TestAppContext) {
        let mut cx = window(cx);
        cx.update(|window, cx| {
            preferences::update(window, cx, |p| p.developer_tools = Some(false))
        });
        assert_eq!(
            cx.update(|_, cx| cx.global::<Preferences>().developer_tools),
            Some(false)
        );
        cx.update(|window, cx| {
            preferences::update(window, cx, |p| p.theme = ThemePreference::Dark)
        });
        assert_eq!(
            cx.update(|_, cx| cx.global::<Preferences>().theme),
            ThemePreference::Dark
        );
    }
}
