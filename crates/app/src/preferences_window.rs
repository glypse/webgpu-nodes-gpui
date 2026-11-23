//! The preferences window: the user's own settings, kept on this computer and
//! shared by every project. Project settings live in the Properties area.

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::{Disableable as _, Sizable as _};
use guard::SafeListener as _;
use theme::{ActiveTheme as _, IconName};

use crate::preferences::{self, Preferences, ThemePreference};
use crate::properties::{field, segmented, segmented_with_icons};

/// An installed font family, drawn in its own face.
#[derive(Clone)]
struct FontChoice(SharedString);

impl SelectItem for FontChoice {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.0.clone()
    }

    fn display_title(&self) -> Option<gpui::AnyElement> {
        Some(
            div()
                .font_family(self.0.clone())
                .child(self.0.clone())
                .into_any_element(),
        )
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().font_family(self.0.clone()).child(self.0.clone())
    }

    fn value(&self) -> &SharedString {
        &self.0
    }
}

/// Installed family names, deduplicated and sorted case-insensitively. Names starting
/// with a dot are the platform's hidden system faces.
fn family_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names: Vec<String> = names
        .into_iter()
        .filter(|name| !name.trim().is_empty() && !name.starts_with('.'))
        .collect();
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    names.dedup();
    names
}

/// The preferences window's contents.
pub struct PreferencesDialog {
    focus: FocusHandle,
    font: Entity<SelectState<SearchableVec<FontChoice>>>,
    _subscriptions: Vec<Subscription>,
}

impl PreferencesDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = cx.global::<Preferences>().font_family();
        let mut families = family_names(cx.text_system().all_font_names());
        // A saved family that isn't installed here stays visible instead of silently changing.
        if let Some(name) = &current
            && !families.contains(name)
        {
            families.insert(0, name.clone());
        }
        let selected = current
            .as_ref()
            .and_then(|name| families.iter().position(|f| f == name));
        let items = SearchableVec::new(
            families
                .into_iter()
                .map(|f| FontChoice(f.into()))
                .collect::<Vec<_>>(),
        );
        let font = cx.new(|cx| {
            SelectState::new(
                items,
                selected.map(gpui_component::IndexPath::new),
                window,
                cx,
            )
            .searchable(true)
        });
        let subscriptions = vec![cx.subscribe_in(
            &font,
            window,
            |_, _, event: &SelectEvent<SearchableVec<FontChoice>>, window, cx| {
                let SelectEvent::Confirm(choice) = event;
                let name = choice.as_ref().map(ToString::to_string).unwrap_or_default();
                guard::guard("interface font", || {
                    preferences::update(window, cx, |p| p.font = name);
                });
            },
        )];
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
                Some("Any installed font. Reset for the default.".to_owned()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(320.0)).child(
                            Select::new(&self.font)
                                .placeholder(theme::INTERFACE_FONT)
                                .search_placeholder("Search fonts"),
                        ),
                    )
                    .child(
                        Button::new("reset-font")
                            .ghost()
                            .small()
                            .label("Reset")
                            .disabled(current.font.is_empty())
                            .on_click(cx.safe_listener(|this, _: &ClickEvent, window, cx| {
                                this.font
                                    .update(cx, |f, cx| f.set_selected_index(None, window, cx));
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

    #[test]
    fn family_names_are_deduplicated_and_sorted_ignoring_case() {
        let names =
            ["zeta", "Alpha", "beta", "alpha", "Alpha", ".SF", "", "Beta"].map(String::from);
        assert_eq!(
            family_names(names),
            ["Alpha", "alpha", "Beta", "beta", "zeta"]
        );
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
