use guard::SafeListener as _;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use shader_graph::catalog::{AddMenuEntry, SearchResult, group_by_heading, search_entries};
use theme::{ActiveTheme as _, InputEvent, TextInput, popup};

pub enum AddMenuEvent {
    Picked(Box<AddMenuEntry>),
    DeleteGroup(String),
    Dismissed,
}

/// Node search: headings opening their entries, replaced by a ranked list
/// of matches as soon as something is typed.
pub struct AddMenu {
    entries: Vec<AddMenuEntry>,
    at: Point<Pixels>,
    input: Entity<TextInput>,
    query: String,
    results: Vec<SearchResult>,
    /// The result Enter picks, moved with the arrow keys.
    highlighted: usize,
    open_heading: Option<String>,
    accepts: Box<dyn Fn(&AddMenuEntry) -> bool>,
    focused: bool,
}

impl EventEmitter<AddMenuEvent> for AddMenu {}

impl Focusable for AddMenu {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
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
        let placeholder = placeholder.into();
        let input = cx.new(|cx| TextInput::new(cx).placeholder(placeholder));
        cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.query = input.read(cx).value().to_string();
                this.results = search_entries(&this.entries, &this.query, &*this.accepts);
                this.highlighted = 0;
                cx.notify();
            }
            InputEvent::Submit => this.pick_highlighted(cx),
            InputEvent::Cancel => cx.emit(AddMenuEvent::Dismissed),
            InputEvent::Blur => {}
        })
        .detach();
        Self {
            entries,
            at,
            input,
            query: String::new(),
            results: Vec::new(),
            highlighted: 0,
            open_heading: None,
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

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
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
        if !self.focused {
            self.focused = true;
            self.input.update(cx, |input, cx| input.focus(window, cx));
        }
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let (accent, accent_fg, fg, muted) = (c.accent, c.accent_foreground, c.popover_foreground, c.muted_foreground);
        let item = |id: SharedString, entry: &AddMenuEntry, content: gpui::AnyElement, selected: bool, cx: &mut Context<Self>| {
            let picked = entry.clone();
            let disabled = entry.disabled;
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
                .when(disabled, |d| d.opacity(0.5))
                .when(!disabled, |d| d.cursor_pointer().hover(move |s| s.bg(accent).text_color(accent_fg)))
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
                    if !disabled {
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
        } else {
            let groups = group_by_heading(&self.entries);
            let open = self.open_heading.clone().or_else(|| groups.first().map(|g| g.0.clone()));
            let headings = groups.iter().map(|(heading, _)| {
                let name = heading.clone();
                let selected = Some(heading) == open.as_ref();
                div()
                    .id(SharedString::from(format!("heading-{heading}")))
                    .flex()
                    .justify_between()
                    .px_1p5()
                    .py_1()
                    .rounded(theme.radius.md)
                    .text_size(theme.text.sm)
                    .text_color(fg)
                    .cursor_pointer()
                    .when(selected, |d| d.bg(accent).text_color(accent_fg))
                    .on_mouse_move(cx.safe_listener({
                        let name = name.clone();
                        move |this, _, _, cx| {
                            if this.open_heading.as_ref() != Some(&name) {
                                this.open_heading = Some(name.clone());
                                cx.notify();
                            }
                        }
                    }))
                    .child(heading.clone())
                    .child("›")
            }).collect::<Vec<_>>();
            let entries: Vec<_> = groups
                .iter()
                .find(|(h, _)| Some(h) == open.as_ref())
                .map(|(_, list)| list.iter().map(|e| (*e).clone()).collect())
                .unwrap_or_default();
            let list = entries.iter().enumerate().map(|(i, e): (usize, &AddMenuEntry)| {
                let content = div().truncate().child(e.label.clone()).into_any_element();
                item(format!("entry-{i}").into(), e, content, false, cx)
            }).collect::<Vec<_>>();
            div()
                .flex()
                .gap_1()
                .max_h(px(360.0))
                .child(div().id("headings").w(px(120.0)).flex_none().overflow_y_scroll().children(headings))
                .child(div().id("entries").w(px(190.0)).overflow_y_scroll().children(list))
                .when(self.entries.is_empty(), |d| d.child(div().px_1p5().text_color(muted).child("Nothing fits here.")))
                .into_any_element()
        };

        let width = if self.searching() { px(340.0) } else { px(330.0) };
        let this = cx.entity().downgrade();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |_, cx| cx.emit(AddMenuEvent::Dismissed)).ok();
        };
        let content = div()
            .w(width)
            .p_1()
            .on_key_down(cx.safe_listener(|this, e, _, cx| this.key_down(e, cx)))
            .child(div().px_1().pb_1().border_b_1().border_color(c.border).child(self.input.clone()))
            .child(div().pt_1().child(body));
        popup::panel(cx, self.at, dismiss, content)
    })
    }
}

