use std::collections::{BTreeMap, HashSet};

use super::{ColorMode, convert_color_mode};
use crate::graph::GroupWidget;
use crate::types::{EnumVariant, InputValue, enum_value};

/// The values to set on a colour widget's components when its mode changes
/// from what `before` held to what `after` holds, so the colour stays the
/// same (unlike Blender's Combine Color, which keeps the numbers). `None`
/// when the mode didn't change, or a component is linked: a linked one's
/// value is only known in the shader.
pub fn color_mode_conversion(
    widget: &GroupWidget,
    variants: &[EnumVariant],
    defaults: &BTreeMap<String, InputValue>,
    before: &BTreeMap<String, InputValue>,
    after: &BTreeMap<String, InputValue>,
    linked: &HashSet<String>,
) -> Option<Vec<(String, f64)>> {
    let GroupWidget::Color { mode, modes } = widget;
    let mode_of = |stored: &BTreeMap<String, InputValue>| {
        let value = enum_value(stored.get(mode).or_else(|| defaults.get(mode)), variants)?;
        modes.get(&value.to_string())
    };
    let (from, to) = (mode_of(before)?, mode_of(after)?);
    if from == to
        || from
            .components
            .iter()
            .chain(&to.components)
            .any(|c| linked.contains(c))
    {
        return None;
    }
    let (from_space, to_space) = (ColorMode::parse(&from.space)?, ColorMode::parse(&to.space)?);
    let read = |c: &String| {
        after
            .get(c)
            .or_else(|| defaults.get(c))
            .map_or(0.0, |v| v.component(0))
    };
    let [x, y, z] = &from.components;
    let next = convert_color_mode(from_space, to_space, [read(x), read(y), read(z)]);
    Some(to.components.iter().cloned().zip(next).collect())
}
