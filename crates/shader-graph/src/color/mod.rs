//! Colour spaces, conversions and the gamut mapping the colour picker and
//! the standard library's colour groups share, with what the Color group needs
//! of them.

mod widget;

pub use graph_math::color::*;
pub use widget::color_mode_conversion;

#[cfg(test)]
mod tests;
