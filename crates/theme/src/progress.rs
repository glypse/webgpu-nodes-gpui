use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, div, px, relative,
};

use crate::ActiveTheme as _;

/// A thin bar filled in proportion to `value` out of `max`.
#[derive(IntoElement)]
pub struct Progress {
    value: f64,
    max: f64,
}

impl Progress {
    pub fn new(value: f64) -> Self {
        Self { value, max: 100.0 }
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = max;
        self
    }

    /// How much of the bar is filled, in [0, 1].
    fn fraction(&self) -> f32 {
        if self.max > 0.0 && self.value.is_finite() {
            (self.value / self.max).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("Progress", || {
            let c = &cx.theme().colors;
            let fill = div()
                .h_full()
                .w(relative(self.fraction()))
                .rounded_full()
                .bg(c.primary);
            #[cfg(test)]
            let fill = gpui::InteractiveElement::debug_selector(fill, || "progress-fill".into());
            div()
                .flex()
                .w_full()
                .h(px(4.0))
                .rounded_full()
                .overflow_hidden()
                .bg(c.muted)
                .child(fill)
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Render, TestAppContext};

    use super::*;

    struct Host(f64, f64);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            guard::render("Host", || {
                div().w(px(200.0)).child(Progress::new(self.0).max(self.1))
            })
        }
    }

    fn fill_width(cx: &mut TestAppContext, value: f64, max: f64) -> f32 {
        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(move |_, _| Host(value, max));
        cx.run_until_parked();
        cx.debug_bounds("progress-fill")
            .map_or(0.0, |b| f32::from(b.size.width))
    }

    #[gpui::test]
    fn fills_in_proportion_to_its_value(cx: &mut TestAppContext) {
        assert_eq!(fill_width(cx, 25.0, 100.0), 50.0);
        assert_eq!(fill_width(cx, 3.0, 4.0), 150.0);
    }

    #[gpui::test]
    fn stays_within_the_bar(cx: &mut TestAppContext) {
        assert_eq!(fill_width(cx, 500.0, 100.0), 200.0);
        assert_eq!(fill_width(cx, -5.0, 100.0), 0.0);
        assert_eq!(fill_width(cx, 5.0, 0.0), 0.0);
    }
}
