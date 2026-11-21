//! Design tokens and the UI components the editor is built from.
//!
//! Call `init` once at startup, build the application with `Assets`, and read
//! the theme with `cx.theme()`.

mod button;
mod checkbox;
mod color;
mod color_picker;
mod color_wheels;
mod curve_editor;
mod dialog;
mod gamut_image;
mod icon;
mod input;
mod kbd;
mod menu;
mod number_field;
mod popover;
mod progress;
mod radio_group;
mod ramp_editor;
mod select;
mod separator;
mod textarea;
mod theme;
mod toggle_group;
mod tooltip;

use std::borrow::Cow;

use gpui::App;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use checkbox::Checkbox;
pub use color::{oklch, oklch_a};
pub use color_picker::{ColorPicker, ColorPickerEvent, linear_to_hsla};
pub use color_wheels::{
    ColorWheels, ColorWheelsEvent, oklch_max_chroma, wheel_cursor, wheel_polar,
};
pub use curve_editor::{CurveEditor, CurveEditorEvent, PathStep, curve_path};
pub use dialog::{Dialog, DialogSide};
pub use icon::{Assets, Icon, IconName};
pub use input::{InputEvent, TextInput};
pub use kbd::Kbd;
pub use menu::{Menu, MenuItem};
pub use number_field::{NumberEvent, NumberField};
pub use popover::Popover;
pub use progress::Progress;
pub use radio_group::RadioGroup;
pub use ramp_editor::{RampEditor, RampEditorEvent};
pub use select::{Select, SelectEvent};
pub use separator::Separator;
pub use textarea::Textarea;
pub use theme::{ActiveTheme, Appearance, Colors, INTERFACE_FONT, Radii, TextSizes, Theme};
pub use toggle_group::ToggleGroup;
pub use tooltip::Tooltip;

const INTERFACE_FONT_DATA: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext.ttf");

/// Registers the interface font and the key bindings of the text inputs, dialog and select, and sets
/// the theme to the system appearance.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTERFACE_FONT_DATA)])
        .expect("the bundled font is valid");
    input::bind_keys(cx);
    dialog::bind_keys(cx);
    textarea::bind_keys(cx);
    select::bind_keys(cx);
    menu::bind_keys(cx);
    cx.set_global(Theme::new(Appearance::Dark));
}
