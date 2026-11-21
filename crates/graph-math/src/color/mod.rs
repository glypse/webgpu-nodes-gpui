//! Colours are held as linear sRGB, what shaders compute with, and shown in
//! the colour space of the display.

mod convert;
mod display;
mod modes;

pub use convert::*;
pub use display::*;
pub use modes::*;

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
