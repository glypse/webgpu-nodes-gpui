use guard::SafeListener as _;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Escape, Input, InputEvent, InputState};
use shader_graph::catalog::{AddMenuEntry, MenuSection, SearchResult, group_by_heading_and_section, search_entries};
use theme::popup::{Cascade, Row};
use theme::{ActiveTheme as _, popup};

pub enum AddMenuEvent {
    Picked(Box<AddMenuEntry>),
    DeleteGroup(String),
    Dismissed,
}

/// Node search: the categories as a cascading menu, replaced by a ranked list of matches as soon
/// as something is typed.
pub struct AddMenu {
    entries: Vec<AddMenuEntry>,
    at: Point<Pixels>,
    placeholder: SharedString,
    /// Made on the first render, which is when a window is at hand to build it.
    input: Option<Entity<InputState>>,
    _input_subscription: Option<Subscription>,
    cascade: Option<Entity<Cascade>>,
    _cascade_subscription: Option<Subscription>,
    focus: FocusHandle,
    query: String,
    results: Vec<SearchResult>,
    /// The result Enter picks, moved with the arrow keys.
    highlighted: usize,
    accepts: Box<dyn Fn(&AddMenuEntry) -> bool>,
    focused: bool,
}

impl EventEmitter<AddMenuEvent> for AddMenu {}

impl Focusable for AddMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AddMenu {
    pub fn new(
        entries: Vec<AddMenuEntry>,
        at: Point<Pixels>,
        placeholder: impl Into<SharedString>,
        accepts: Box<dyn Fn(&AddMenuEntry) -> bool>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            entries,
            at,
            placeholder: placeholder.into(),
            input: None,
            _input_subscription: None,
            cascade: None,
            _cascade_subscription: None,
            focus: cx.focus_handle(),
            query: String::new(),
            results: Vec::new(),
            highlighted: 0,
            accepts,
            focused: false,
        }
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(result) = self.results.get(self.highlighted).filter(|r| !r.entry.disabled) {
            cx.emit(AddMenuEvent::Picked(Box::new(result.entry.clone())));
        }
    }

    /// Enter picks the highlighted search result, or acts on the highlighted category row.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.searching() {
            self.pick_highlighted(cx);
        } else {
            self.to_categories("enter".to_owned(), window, cx);
        }
    }

    /// Hands a key to the category menu once this menu is no longer being updated: a pick the
    /// category menu makes updates this menu.
    fn to_categories(&self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cascade) = self.cascade.clone() else { return };
        window.defer(cx, move |window, cx| {
            cascade.update(cx, |c, cx| {
                c.key(&key, window, cx);
            });
        });
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.searching() {
            let key = event.keystroke.key.clone();
            if key != "enter" && key != "escape" {
                self.to_categories(key, window, cx);
            }
            return;
        }
        let count = self.results.len();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => {
                self.highlighted = (self.highlighted + 1) % count;
                cx.notify();
            }
            "up" if count > 0 => {
                self.highlighted = (self.highlighted + count - 1) % count;
                cx.notify();
            }
            _ => {}
        }
    }
}

/// The categories of `entries`: a heading per category, its sections as submenus beside it.
fn category_rows(weak: &gpui::WeakEntity<AddMenu>, entries: &[AddMenuEntry]) -> Vec<Row> {
    group_by_heading_and_section(entries)
        .into_iter()
        .map(|(heading, loose, sections)| Row::submenu(heading, section_rows(weak, &loose, &sections)))
        .collect()
}

fn section_rows(weak: &gpui::WeakEntity<AddMenu>, loose: &[&AddMenuEntry], sections: &[MenuSection<'_>]) -> Vec<Row> {
    let mut rows: Vec<Row> = loose.iter().map(|entry| entry_row(weak, entry)).collect();
    for (index, section) in sections.iter().enumerate() {
        let row = Row::submenu(section.name.clone(), section_rows(weak, &section.entries, &section.sections));
        rows.push(if index == 0 && !loose.is_empty() { row.separated() } else { row });
    }
    rows
}

fn entry_row(weak: &gpui::WeakEntity<AddMenu>, entry: &AddMenuEntry) -> Row {
    let (picked, pick_weak) = (entry.clone(), weak.clone());
    let mut row = Row::pick(
        entry.label.clone(),
        Rc::new(move |_: &mut Window, cx: &mut App| {
            pick_weak.update(cx, |_, cx| cx.emit(AddMenuEvent::Picked(Box::new(picked.clone())))).ok();
        }),
    )
    .disabled(entry.disabled);
    if entry.deletable {
        let (key, delete_weak) = (entry.key.clone(), weak.clone());
        row = row.trailing(
            "delete",
            Rc::new(move |_: &mut Window, cx: &mut App| {
                delete_weak.update(cx, |_, cx| cx.emit(AddMenuEvent::DeleteGroup(key.clone()))).ok();
            }),
        );
    }
    row
}

fn highlighted(text: &str, ranges: &[(usize, usize)], muted: gpui::Hsla, strong: gpui::Hsla) -> gpui::Div {
    let mut parts = div().flex().flex_none().whitespace_nowrap();
    let mut at = 0;
    for &(start, end) in ranges {
        let (Some(before), Some(matched)) = (text.get(at.min(start)..start), text.get(start..end)) else {
            log::warn!("ignoring highlight {start}..{end} outside {text:?}");
            continue;
        };
        if !before.is_empty() {
            parts = parts.child(div().text_color(muted).child(before.to_owned()));
        }
        if !matched.is_empty() {
            parts = parts.child(div().text_color(strong).font_weight(FontWeight::SEMIBOLD).child(matched.to_owned()));
        }
        at = at.max(end);
    }
    if let Some(rest) = text.get(at..).filter(|r| !r.is_empty()) {
        parts = parts.child(div().text_color(muted).child(rest.to_owned()));
    }
    parts
}

impl Render for AddMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("add menu", || {
        let input = match self.input.clone() {
            Some(input) => input,
            None => {
                let placeholder = self.placeholder.clone();
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                self._input_subscription = Some(cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.query = input.read(cx).value().to_string();
                            this.results = search_entries(&this.entries, &this.query, &*this.accepts);
                            this.highlighted = 0;
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.enter(window, cx),
                        _ => {}
                    }
                }));
                self.input = Some(input.clone());
                input
            }
        };
        if self.cascade.is_none() {
            let rows = category_rows(&cx.entity().downgrade(), &self.entries);
            let cascade = cx.new(|cx| Cascade::new(rows, px(264.0), Vec::new(), cx));
            self._cascade_subscription = Some(cx.subscribe(&cascade, |_, _, _: &DismissEvent, cx| {
                cx.emit(AddMenuEvent::Dismissed);
            }));
            self.cascade = Some(cascade);
        }
        if !self.focused {
            self.focused = true;
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let (accent, accent_fg, fg, muted) = (c.accent, c.accent_foreground, c.popover_foreground, c.muted_foreground);
        let item = |id: SharedString, entry: &AddMenuEntry, content: gpui::AnyElement, selected: bool, cx: &mut Context<Self>| {
            let picked = entry.clone();
            let deletable = entry.deletable;
            let key = entry.key.clone();
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .gap_1p5()
                .px_1p5()
                .py_1()
                .rounded(theme.radius.md)
                .text_size(theme.text.sm)
                .text_color(fg)
                .when(selected, |d| d.bg(accent).text_color(accent_fg))
                .when(entry.disabled, |d| d.opacity(0.5))
                .when(!entry.disabled, |d| d.cursor_pointer().hover(move |s| s.bg(accent).text_color(accent_fg)))
                .child(content)
                .when(deletable, |d| {
                    d.child(
                        div()
                            .id("delete")
                            .px_1()
                            .rounded(theme.radius.sm)
                            .text_size(theme.text.xs)
                            .text_color(c.destructive)
                            .hover(move |s| s.bg(c.destructive.opacity(0.15)))
                            .on_mouse_down(gpui::MouseButton::Left, cx.safe_listener({
                                let key = key.clone();
                                move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(AddMenuEvent::DeleteGroup(key.clone()));
                                }
                            }))
                            .child("delete"),
                    )
                })
                .on_click(cx.safe_listener(move |_, _: &ClickEvent, _, cx| {
                    if !picked.disabled {
                        cx.emit(AddMenuEvent::Picked(Box::new(picked.clone())));
                    }
                }))
        };

        let body = if self.searching() {
            let rows: Vec<_> = self
                .results
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let label = highlighted(&r.entry.label, &r.label_ranges, muted, fg);
                    let content = div()
                        .flex()
                        .gap_1()
                        .min_w_0()
                        .child(label)
                        .children(r.alias.as_ref().map(|a| {
                            div()
                                .flex()
                                .text_color(muted)
                                .child("(")
                                .child(highlighted(a, &r.alias_ranges, muted, fg))
                                .child(")")
                        }))
                        .child(div().flex_1())
                        .child(div().text_size(theme.text.xs).text_color(muted).child(r.entry.heading.clone()))
                        .into_any_element();
                    item(format!("result-{i}").into(), &r.entry, content, i == self.highlighted, cx)
                })
                .collect();
            div()
                .id("results")
                .max_h(px(320.0))
                .overflow_y_scroll()
                .children(rows)
                .when(self.results.is_empty(), |d| {
                    d.child(div().px_1p5().py_1p5().text_size(theme.text.sm).text_color(muted).child("No matching node."))
                })
                .into_any_element()
        } else if self.entries.is_empty() {
            div().px_1p5().text_color(muted).child("Nothing fits here.").into_any_element()
        } else {
            self.cascade.clone().map_or_else(|| div().into_any_element(), |c| c.into_any_element())
        };

        let width = if self.searching() { px(340.0) } else { px(280.0) };
        let this = cx.entity().downgrade();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |_, cx| cx.emit(AddMenuEvent::Dismissed)).ok();
        };
        let content = div()
            .w(width)
            .p_1()
            .track_focus(&self.focus)
            .on_key_down(cx.safe_listener(|this, e, window, cx| this.key_down(e, window, cx)))
            .on_action(cx.safe_listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                if this.searching() {
                    cx.emit(AddMenuEvent::Dismissed);
                } else {
                    this.to_categories("escape".to_owned(), window, cx);
                }
            }))
            .child(div().px_1().pb_1().border_b_1().border_color(c.border).child(Input::new(&input)))
            .child(div().pt_1().child(body));
        popup::layer(window, cx, self.at, dismiss, content)
    })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Render, TestAppContext, VisualTestContext, point, px};
    use shader_graph::catalog::EntryKind;

    use super::*;

    fn entry(label: &str) -> AddMenuEntry {
        AddMenuEntry {
            key: label.to_owned(),
            label: label.to_owned(),
            aliases: Vec::new(),
            heading: "Math".to_owned(),
            section: Vec::new(),
            search_section: None,
            kind: EntryKind::Node { node_type: label.to_owned() },
            data: None,
            choices: Vec::new(),
            disabled: false,
            deletable: false,
        }
    }

    fn in_section(label: &str, section: &str) -> AddMenuEntry {
        AddMenuEntry { section: vec![section.to_owned()], ..entry(label) }
    }

    struct Host(Entity<AddMenu>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            guard::render("add menu host", || self.0.clone())
        }
    }

    type Events = Rc<RefCell<Vec<String>>>;

    fn setup_with(cx: &mut TestAppContext, entries: Vec<AddMenuEntry>) -> (Entity<AddMenu>, Events, &mut VisualTestContext) {
        cx.update(theme::init);
        let menu = cx.update(|cx| {
            cx.new(|cx| {
                AddMenu::new(entries, point(px(0.0), px(0.0)), "Search nodes…", Box::new(|_| true), cx)
            })
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        cx.update(|cx| {
            cx.subscribe(&menu, move |_, event: &AddMenuEvent, _| {
                log.borrow_mut().push(match event {
                    AddMenuEvent::Picked(entry) => format!("picked {}", entry.label),
                    AddMenuEvent::DeleteGroup(key) => format!("delete {key}"),
                    AddMenuEvent::Dismissed => "dismissed".to_owned(),
                });
            })
            .detach();
        });
        let (_, cx) = cx.add_window_view(|_, _| Host(menu.clone()));
        cx.run_until_parked();
        (menu, events, cx)
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<AddMenu>, Events, &mut VisualTestContext) {
        setup_with(cx, vec![entry("Sine"), entry("Cosine"), entry("Add")])
    }

    fn path(menu: &Entity<AddMenu>, cx: &mut VisualTestContext) -> Vec<usize> {
        cx.read(|cx| {
            let cascade = menu.read(cx).cascade.clone().expect("the categories are made");
            cascade.read(cx).open_path().to_vec()
        })
    }

    fn frame(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn hover(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"));
        cx.simulate_mouse_move(bounds.center(), None, gpui::Modifiers::none());
        cx.executor().advance_clock(std::time::Duration::from_millis(400));
        frame(cx);
    }

    #[gpui::test]
    fn typing_searches_and_the_arrows_and_enter_pick_from_the_results(cx: &mut TestAppContext) {
        let (menu, events, cx) = setup(cx);
        cx.simulate_input("sin");
        cx.run_until_parked();
        let labels: Vec<String> =
            cx.read(|cx| menu.read(cx).results.iter().map(|r| r.entry.label.clone()).collect());
        assert_eq!(labels, ["Sine", "Cosine"]);

        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| menu.read(cx).highlighted), 1);

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["picked Cosine"]);
    }

    #[gpui::test]
    fn escape_dismisses_the_menu(cx: &mut TestAppContext) {
        let (_, events, cx) = setup(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["dismissed"]);
    }

    #[gpui::test]
    fn hovering_a_category_opens_its_entries_and_keeps_the_categories(cx: &mut TestAppContext) {
        let (menu, _, cx) = setup_with(cx, vec![entry("Sine"), entry("Cosine")]);
        frame(cx);
        hover(cx, "menu-row Math");
        assert_eq!(path(&menu, cx), [0]);
        assert!(cx.debug_bounds("menu-row Sine").is_some(), "the category's entries are drawn");
    }

    #[gpui::test]
    fn sections_of_a_category_open_as_submenus(cx: &mut TestAppContext) {
        let (menu, events, cx) =
            setup_with(cx, vec![entry("Sine"), in_section("Fold", "Helpers"), in_section("Wrap", "Helpers")]);
        frame(cx);
        hover(cx, "menu-row Math");
        hover(cx, "menu-row Helpers");
        assert_eq!(path(&menu, cx), [0, 1]);
        assert!(cx.debug_bounds("menu-row Sine").is_some(), "the parent category stays open");
        hover(cx, "menu-row Wrap");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["picked Wrap", "dismissed"]);
    }

    #[gpui::test]
    fn the_keys_walk_the_categories(cx: &mut TestAppContext) {
        let (menu, events, cx) =
            setup_with(cx, vec![entry("Sine"), in_section("Fold", "Helpers")]);
        frame(cx);
        cx.simulate_keystrokes("down right");
        frame(cx);
        assert_eq!(path(&menu, cx), [0]);
        cx.simulate_keystrokes("down right");
        frame(cx);
        assert_eq!(path(&menu, cx), [0, 1]);
        cx.simulate_keystrokes("left escape");
        frame(cx);
        assert_eq!(path(&menu, cx), Vec::<usize>::new());
        assert!(events.borrow().is_empty(), "closing submenus does not dismiss the menu");
        cx.simulate_keystrokes("down enter");
        frame(cx);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["picked Sine", "dismissed"]);
    }
}
