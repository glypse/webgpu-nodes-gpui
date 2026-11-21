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
mod input;
mod number_field;
mod ramp_editor;
pub mod popup;
mod textarea;
mod theme;

use std::borrow::Cow;

use gpui::App;

pub use color::{oklch, oklch_a};
pub use color_picker::{ColorPicker, ColorPickerEvent, linear_to_hsla};
pub use color_wheels::{
    ColorWheels, ColorWheelsEvent, oklch_max_chroma, wheel_cursor, wheel_polar,
};
pub use curve_editor::{CurveEditor, CurveEditorEvent, PathStep, curve_path};
pub use icon::{Assets, Icon, IconName};
pub use input::{InputEvent, TextInput};
pub use number_field::{NumberEvent, NumberField};
pub use ramp_editor::{RampEditor, RampEditorEvent};
pub use textarea::Textarea;
pub use theme::{ActiveTheme, Appearance, Colors, INTERFACE_FONT, Radii, TextSizes, Theme};

const INTERFACE_FONT_DATA: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext.ttf");

/// Registers the interface font and the key bindings of the text inputs and textarea, and sets the
/// theme to the system appearance.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTERFACE_FONT_DATA)])
        .expect("the bundled font is valid");
    input::bind_keys(cx);
    textarea::bind_keys(cx);
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
