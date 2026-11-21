//! Event handlers run inside platform callbacks that cannot unwind: a panic there aborts the
//! whole process. These helpers stop the unwind at our own boundary and log it instead.

use std::backtrace::Backtrace;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{App, Context, Window};

/// Routes panics through `log` with their location and a backtrace (captured when
/// `RUST_BACKTRACE` asks for one), replacing the default stderr hook.
pub fn install_panic_hook() {
    panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info.location().map(|l| l.to_string()).unwrap_or_default();
        log::error!("panic at {location}: {message}\n{}", Backtrace::capture());
    }));
}

static RECOVERED: AtomicUsize = AtomicUsize::new(0);

/// How many panics [`guard`] has caught so far.
pub fn recovered() -> usize {
    RECOVERED.load(Ordering::Relaxed)
}

/// Runs `f`, turning a panic into a logged error and `None`.
///
/// State `f` mutated before panicking is kept as is: callers accept a possibly half-applied
/// change over losing the process.
pub fn guard<R>(context: &str, f: impl FnOnce() -> R) -> Option<R> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(_) => {
            RECOVERED.fetch_add(1, Ordering::Relaxed);
            log::error!("recovered from a panic in {context}");
            None
        }
    }
}

pub trait SafeListener<T: 'static> {
    /// [`Context::listener`] whose body is wrapped in [`guard`].
    fn safe_listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static;
}

impl<T: 'static> SafeListener<T> for Context<'_, T> {
    fn safe_listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        self.listener(move |this, event, window, cx| {
            guard(std::any::type_name::<T>(), || f(this, event, window, cx));
        })
    }
}

/// Runs a view's render body under [`guard`]; a panic draws a placeholder for this frame.
pub fn render<E: gpui::IntoElement>(context: &str, f: impl FnOnce() -> E) -> gpui::AnyElement {
    use gpui::{IntoElement as _, ParentElement as _, Styled as _};
    guard(context, || f().into_any_element()).unwrap_or_else(|| {
        gpui::div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui::red())
            .child(format!("{context} failed to render; see the log"))
            .into_any_element()
    })
}
