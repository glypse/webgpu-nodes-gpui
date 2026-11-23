//! Picking a type from a menu organised like the add menu: one entry per
//! scalar kind (float, integer, unsigned, boolean) holding its scalar and
//! vector widths, then the textures when offered.

use std::rc::Rc;

use gpui::{App, Context, WeakEntity};
use shader_graph::types::{ScalarKind, WgslType};

use crate::editor::{Editor, ListItem, OnSelect};

pub type Pick = Rc<dyn Fn(WgslType, &mut Editor, &mut Context<Editor>)>;
pub type Reset = Rc<dyn Fn(&mut Editor, &mut Context<Editor>)>;

pub struct TypePicker {
    /// Selectable types.
    pub types: Vec<WgslType>,
    /// The current type, checked in the menu.
    pub value: Option<WgslType>,
    /// The type links infer: offered first as the way back to inference, which `on_reset` takes.
    pub inferred: Option<WgslType>,
    pub on_pick: Pick,
    pub on_reset: Option<Reset>,
    /// Entries listed first, before the types.
    pub extra: Vec<ListItem>,
}

const KINDS: [(ScalarKind, &str); 4] = [
    (ScalarKind::F32, "Float"),
    (ScalarKind::I32, "Integer"),
    (ScalarKind::U32, "Unsigned integer"),
    (ScalarKind::Bool, "Boolean"),
];

const WIDTHS: [&str; 5] = ["", "Scalar", "Vector 2", "Vector 3", "Vector 4"];

/// Wraps a handler on the editor as a list entry's.
pub fn on_editor(weak: &WeakEntity<Editor>, f: impl Fn(&mut Editor, &mut Context<Editor>) + 'static) -> OnSelect {
    let weak = weak.clone();
    Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
        super::update_editor(&weak, cx, |e, cx| f(e, cx));
    })
}

fn type_item(weak: &WeakEntity<Editor>, picker: &Rc<TypePicker>, ty: WgslType) -> ListItem {
    let pick = picker.on_pick.clone();
    ListItem::new(ty.id(), Some(picker.value == Some(ty)), on_editor(weak, move |e, cx| pick(ty, e, cx)))
        .detail(WIDTHS.get(ty.components()).copied().unwrap_or_default())
}

/// The entries of a picker's menu; kinds with several types open a submenu of them.
pub fn items(weak: &WeakEntity<Editor>, picker: Rc<TypePicker>) -> Vec<ListItem> {
    let mut items = picker.extra.clone();
    if let Some(inferred) = picker.inferred {
        let reset = picker.on_reset.clone();
        let mut item = ListItem::new(
            inferred.id(),
            Some(picker.value.is_none()),
            on_editor(weak, move |e, cx| {
                if let Some(reset) = &reset {
                    reset(e, cx);
                }
            }),
        )
        .detail("Inferred");
        if !items.is_empty() {
            item = item.separated();
        }
        items.push(item);
    }
    let values: Vec<WgslType> = picker.types.iter().copied().filter(|t| !t.is_texture()).collect();
    let groups: Vec<(&str, ScalarKind, Vec<WgslType>)> = KINDS
        .iter()
        .filter_map(|&(kind, label)| {
            let mut members: Vec<WgslType> = values.iter().copied().filter(|t| t.kind() == kind).collect();
            members.sort_by_key(|t| t.components());
            (!members.is_empty()).then_some((label, kind, members))
        })
        .collect();
    let first_of_values = items.len();
    if groups.len() == 1 {
        items.extend(groups[0].2.iter().map(|&t| type_item(weak, &picker, t)));
    } else {
        for (label, kind, members) in groups {
            if let [only] = members[..] {
                items.push(type_item(weak, &picker, only));
                continue;
            }
            let current = picker.value.filter(|v| !v.is_texture() && v.kind() == kind);
            let sub: Vec<ListItem> = members.iter().map(|&t| type_item(weak, &picker, t)).collect();
            let mut entry = ListItem::group(label, sub);
            if let Some(current) = current {
                entry = entry.detail(current.id());
            }
            if first_of_values == items.len() && !items.is_empty() {
                entry = entry.separated();
            }
            items.push(entry);
        }
    }
    if let Some(&texture) = picker.types.iter().find(|t| t.is_texture()) {
        let mut item = type_item(weak, &picker, texture).detail("Image");
        if items.len() > first_of_values || !items.is_empty() {
            item = item.separated();
        }
        items.push(item);
    }
    items
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px};
    use shader_graph::types::WgslType;

    use crate::editor::Editor;

    fn frame(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn hovering_a_kind_opens_its_types_beside_it_and_the_kinds_stay(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (editor, cx) = cx.add_window_view(Editor::new);
        editor.update(cx, |e, cx| e.open_type_list(point(px(300.0), px(120.0)), "none".to_owned(), true, cx));
        frame(cx);
        let kind = cx.debug_bounds("menu-row Float").expect("the kinds are drawn");
        cx.simulate_mouse_move(kind.center(), None, Modifiers::none());
        cx.executor().advance_clock(Duration::from_millis(400));
        frame(cx);

        let type_row: &'static str = Box::leak(format!("menu-row {}", WgslType::F32.id()).into_boxed_str());
        assert!(cx.debug_bounds(type_row).is_some(), "the float types open beside Float");
        assert!(cx.debug_bounds("menu-row Integer").is_some(), "the other kinds stay open");
    }
}
