//! Design tokens and the UI components the editor is built from.
//!
//! Call `init` once at startup, build the application with `Assets`, and read
//! the theme with `cx.theme()`.

mod color;
mod color_picker;
mod color_wheels;
mod curve_editor;
mod gamut_image;
mod icon;
mod number_field;
pub mod popup;
mod ramp_editor;
mod theme;

use std::borrow::Cow;

use gpui::{AnyElement, App, IntoElement as _};
use gpui_component::button::Button;

pub use color::{oklch, oklch_a};
pub use color_picker::{ColorPicker, ColorPickerEvent, linear_to_hsla};
pub use color_wheels::{
    ColorWheels, ColorWheelsEvent, oklch_max_chroma, wheel_cursor, wheel_polar,
};
pub use curve_editor::{CurveEditor, CurveEditorEvent, PathStep, curve_path};
pub use icon::{Assets, Icon, IconName};
pub use number_field::{NumberEvent, NumberField};
pub use ramp_editor::{RampEditor, RampEditorEvent};
pub use theme::{ActiveTheme, Appearance, Colors, INTERFACE_FONT, Radii, TextSizes, Theme};

/// A button under a test id, so tests can find it. Outside tests it is the
/// button itself, keeping the layout free of an extra element.
pub(crate) fn tagged_button(button: Button, id: impl FnOnce() -> String) -> AnyElement {
    #[cfg(test)]
    {
        use gpui::{InteractiveElement as _, ParentElement as _, div};
        div().debug_selector(id).child(button).into_any_element()
    }
    #[cfg(not(test))]
    {
        let _ = id;
        button.into_any_element()
    }
}

const INTERFACE_FONT_DATA: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext.ttf");

/// Registers the interface font and gpui-component, and sets the theme to the system appearance.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTERFACE_FONT_DATA)])
        .expect("the bundled font is valid");
    cx.set_global(Theme::new(Appearance::Dark));
    sync_component_theme(Appearance::Dark, cx);
}

/// Sets gpui-component's light or dark palette, which its own widgets read, to match ours.
pub fn sync_component_theme(appearance: Appearance, cx: &mut App) {
    let mode = match appearance {
        Appearance::Light => gpui_component::theme::ThemeMode::Light,
        Appearance::Dark => gpui_component::theme::ThemeMode::Dark,
    };
    gpui_component::theme::Theme::change(mode, None, cx);
}
