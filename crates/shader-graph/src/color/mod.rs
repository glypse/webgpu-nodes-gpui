//! Colour spaces, conversions and the gamut mapping the colour picker and
//! the standard library's colour groups share.

mod convert;
mod display;
mod modes;
mod widget;

pub use convert::*;
pub use display::*;
pub use modes::*;
pub use widget::color_mode_conversion;

use serde::{Deserialize, Serialize};

/// The colour space frames are presented and exported in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorSpace {
    #[default]
    Srgb,
    DisplayP3,
}

impl ColorSpace {
    pub const ALL: [ColorSpace; 2] = [ColorSpace::Srgb, ColorSpace::DisplayP3];
}

#[cfg(test)]
mod tests;
