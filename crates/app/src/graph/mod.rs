pub mod add_menu;
pub mod breadcrumbs;
pub mod fields;
pub mod frames;
pub mod handle_details;
pub mod item_widgets;
pub mod node;
pub mod popovers;
pub mod sheet;
pub mod socket;
pub mod type_picker;
pub mod type_set_picker;
pub mod widgets;
pub mod zones;

use gpui::{App, Context, WeakEntity};

use crate::editor::Editor;

/// Runs an editor update from a UI callback. Does nothing once the editor is gone; a panic
/// is logged instead of unwinding into the platform.
pub(crate) fn update_editor(
    ed: &WeakEntity<Editor>,
    cx: &mut App,
    f: impl FnOnce(&mut Editor, &mut Context<Editor>),
) {
    guard::guard("editor callback", || {
        ed.update(cx, f).ok();
    });
}
