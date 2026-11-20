//! Design tokens and the UI components the editor is built from.
//!
//! Call `init` once at startup, build the application with `Assets`, and read
//! the theme with `cx.theme()`.

mod button;
mod color;
mod icon;
mod input;
mod kbd;
mod menu;
mod number_field;
mod popover;
mod separator;
mod theme;
mod toggle_group;
mod tooltip;

use std::borrow::Cow;

use gpui::App;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use color::{oklch, oklch_a};
pub use icon::{Assets, Icon, IconName};
pub use input::{InputEvent, TextInput};
pub use kbd::Kbd;
pub use menu::{Menu, MenuItem};
pub use number_field::{NumberEvent, NumberField};
pub use popover::Popover;
pub use separator::Separator;
pub use theme::{ActiveTheme, Appearance, Colors, INTERFACE_FONT, Radii, TextSizes, Theme};
pub use toggle_group::ToggleGroup;
pub use tooltip::Tooltip;

const INTERFACE_FONT_DATA: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext.ttf");

/// Registers the interface font and the text input's key bindings, and sets
/// the theme to the system appearance.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTERFACE_FONT_DATA)])
        .expect("the bundled font is valid");
    input::bind_keys(cx);
    cx.set_global(Theme::new(Appearance::Dark));
}
