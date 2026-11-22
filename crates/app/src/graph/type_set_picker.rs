//! Picking the types a type variable may take: a named set, or any of them
//! one by one, and which it takes when nothing decides.

use std::rc::Rc;

use gpui::{Pixels, Point, WeakEntity};
use shader_graph::types::{TYPE_SETS, WgslType, type_set_of, with_default_type};
use gpui::Context;

use super::type_picker::on_editor;
use crate::editor::{Editor, ListItem};

pub type Change = Rc<dyn Fn(Vec<WgslType>, &mut Editor, &mut Context<Editor>)>;

/// The sets listed as kinds to pick types of one by one.
const KINDS: usize = 4;

fn sorted(types: &[WgslType]) -> Vec<WgslType> {
    let mut sorted = types.to_vec();
    sorted.sort_by_key(|t| t.components());
    sorted
}

/// The label of the button that opens the picker.
pub fn label(types: &[WgslType]) -> String {
    let base = type_set_of(types).map_or_else(
        || types.iter().map(|t| t.id()).collect::<Vec<_>>().join(", "),
        |set| set.label.to_owned(),
    );
    match (type_set_of(types), types.first()) {
        (Some(set), Some(default)) if set.types.first() != Some(default) => format!("{base} {}", default.id()),
        _ => base,
    }
}

/// The entries of the picker's menu for a variable that takes `types` (the first the default).
pub fn items(weak: &WeakEntity<Editor>, at: Point<Pixels>, types: Vec<WgslType>, change: Change) -> Vec<ListItem> {
    let default = types.first().copied();
    let current = type_set_of(&types).map(|s| s.id);
    let mut items: Vec<ListItem> = TYPE_SETS
        .iter()
        .map(|set| {
            let (change, next) = (change.clone(), with_default_type(set.types, default));
            ListItem::new(set.label, Some(current == Some(set.id)), on_editor(weak, move |e, cx| change(next.clone(), e, cx)))
        })
        .collect();
    for (index, set) in TYPE_SETS.iter().take(KINDS).enumerate() {
        let chosen = set.types.iter().filter(|t| types.contains(t)).count();
        let sub = kind_items(weak, at, types.clone(), index, change.clone());
        let mut entry = ListItem::group(set.label, sub);
        if chosen > 0 {
            entry = entry.detail(chosen.to_string());
        }
        if index == 0 {
            entry = entry.separated();
        }
        items.push(entry);
    }
    let sub = default_items(weak, types, change);
    let mut entry = ListItem::group("Default", sub).separated();
    if let Some(default) = default {
        entry = entry.detail(default.id());
    }
    items.push(entry);
    items
}

/// One type per entry of a kind. Toggling one changes the variable and reopens the menu with the
/// kind's submenu still open, so the checks show the new types.
fn kind_items(weak: &WeakEntity<Editor>, at: Point<Pixels>, types: Vec<WgslType>, kind: usize, change: Change) -> Vec<ListItem> {
    let Some(set) = TYPE_SETS.get(kind) else { return Vec::new() };
    sorted(set.types)
        .into_iter()
        .map(|ty| {
            let checked = types.contains(&ty);
            let (w, ts, ch) = (weak.clone(), types.clone(), change.clone());
            ListItem::new(
                ty.id(),
                Some(checked),
                on_editor(weak, move |e, cx| {
                    let next: Vec<WgslType> = if checked {
                        ts.iter().copied().filter(|t| *t != ty).collect()
                    } else {
                        ts.iter().copied().chain([ty]).collect()
                    };
                    ch(next.clone(), e, cx);
                    let root = items(&w, at, next, ch.clone());
                    e.open_list_at(at, root, vec![TYPE_SETS.len() + kind], cx);
                }),
            )
            .disabled(checked && types.len() == 1)
        })
        .collect()
}

fn default_items(weak: &WeakEntity<Editor>, types: Vec<WgslType>, change: Change) -> Vec<ListItem> {
    let default = types.first().copied();
    types
        .iter()
        .map(|&ty| {
            let (ts, ch) = (types.clone(), change.clone());
            ListItem::new(ty.id(), Some(Some(ty) == default), on_editor(weak, move |e, cx| ch(with_default_type(&ts, Some(ty)), e, cx)))
        })
        .collect()
}
