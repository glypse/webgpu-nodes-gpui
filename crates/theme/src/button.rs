use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::{ActiveTheme as _, Icon, IconName, Tooltip};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Outline,
    Secondary,
    Ghost,
    Destructive,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    /// Square, for a lone icon.
    Icon,
    IconXs,
    IconSm,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    icon_end: Option<IconName>,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    selected: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    children: Vec<AnyElement>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            icon_end: None,
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            disabled: false,
            selected: false,
            tooltip: None,
            on_click: None,
            children: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_end(mut self, icon: IconName) -> Self {
        self.icon_end = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Shown pressed, as a trigger whose menu is open or a toggled option.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let dark = theme.is_dark();
        let (bg, fg, hover_bg, border) = match self.variant {
            ButtonVariant::Default => (
                c.primary,
                c.primary_foreground,
                c.primary.opacity(0.8),
                None,
            ),
            ButtonVariant::Outline => {
                let bg = if dark {
                    c.input.opacity(0.3)
                } else {
                    c.background
                };
                let hover = if dark { c.input.opacity(0.5) } else { c.muted };
                (
                    bg,
                    c.foreground,
                    hover,
                    Some(if dark { c.input } else { c.border }),
                )
            }
            ButtonVariant::Secondary => (
                c.secondary,
                c.secondary_foreground,
                c.secondary.opacity(0.8),
                None,
            ),
            ButtonVariant::Ghost => (
                gpui::transparent_black(),
                c.foreground,
                if dark { c.muted.opacity(0.5) } else { c.muted },
                None,
            ),
            ButtonVariant::Destructive => {
                let alpha = if dark { 0.2 } else { 0.1 };
                (
                    c.destructive.opacity(alpha),
                    c.destructive,
                    c.destructive.opacity(alpha + 0.1),
                    None,
                )
            }
        };
        let bg = if self.selected { hover_bg } else { bg };
        let small_radius = theme.radius.md.min(px(10.0));
        let (height, pad, text, radius, icon) = match self.size {
            ButtonSize::Xs => (px(24.0), px(8.0), theme.text.xs, small_radius, px(12.0)),
            ButtonSize::Sm => (px(28.0), px(10.0), px(12.8), small_radius, px(14.0)),
            ButtonSize::Default => (px(32.0), px(10.0), theme.text.sm, theme.radius.lg, px(16.0)),
            ButtonSize::Lg => (px(36.0), px(10.0), theme.text.sm, theme.radius.lg, px(16.0)),
            ButtonSize::Icon => (px(32.0), px(0.0), theme.text.sm, theme.radius.lg, px(16.0)),
            ButtonSize::IconXs => (px(24.0), px(0.0), theme.text.xs, small_radius, px(12.0)),
            ButtonSize::IconSm => (px(28.0), px(0.0), theme.text.sm, small_radius, px(16.0)),
        };
        let square = matches!(
            self.size,
            ButtonSize::Icon | ButtonSize::IconXs | ButtonSize::IconSm
        );
        let gap = if matches!(self.size, ButtonSize::Default | ButtonSize::Lg) {
            px(6.0)
        } else {
            px(4.0)
        };
        let disabled = self.disabled;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(gap)
            .h(height)
            .when(square, |d| d.w(height))
            .when(!square, |d| d.px(pad))
            .rounded(radius)
            .border_1()
            .border_color(border.unwrap_or(gpui::transparent_black()))
            .bg(bg)
            .text_color(fg)
            .text_size(text)
            .line_height(relative(1.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| d.hover(move |s| s.bg(hover_bg)))
            .children(self.icon.map(|i| Icon::new(i).size(icon).color(fg)))
            .children(self.label)
            .children(self.children)
            .children(
                self.icon_end
                    .map(|i| Icon::new(i).size(icon * 0.75).color(fg)),
            )
            .when_some(self.tooltip, |d, text| d.tooltip(Tooltip::text(text)))
            .when_some(self.on_click.filter(|_| !disabled), |d, handler| {
                d.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx)
                })
            })
    }
}
