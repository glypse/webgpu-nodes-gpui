//! The model several graph areas share, each viewing it from its own group
//! path with its own selection.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use gpui::WeakEntity;
use shader_graph::graph::GraphModel;

use super::Editor;

/// What an area sees of the shared model.
#[derive(Default)]
struct AreaView {
    path: Vec<String>,
    selected: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct ModelHandle {
    shared: Rc<RefCell<GraphModel>>,
    view: Rc<RefCell<AreaView>>,
    /// Every area's editor, the first being the one the shell listens to.
    areas: Rc<RefCell<Vec<WeakEntity<Editor>>>>,
    /// Bumped whenever the model may have changed, including its path and selection.
    revision: Rc<Cell<u64>>,
}

pub(crate) struct ModelMut<'a> {
    model: RefMut<'a, GraphModel>,
    view: &'a RefCell<AreaView>,
}

impl ModelHandle {
    pub fn new(model: GraphModel) -> Self {
        Self {
            shared: Rc::new(RefCell::new(model)),
            view: Rc::default(),
            areas: Rc::default(),
            revision: Rc::default(),
        }
    }

    /// The same model seen from another area, which starts at the root with nothing selected.
    pub fn another_view(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            view: Rc::default(),
            areas: self.areas.clone(),
            revision: self.revision.clone(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    pub fn register(&self, editor: WeakEntity<Editor>) {
        let mut areas = self.areas.borrow_mut();
        areas.retain(|a| a.upgrade().is_some());
        areas.push(editor);
    }

    pub fn areas(&self) -> Vec<WeakEntity<Editor>> {
        self.areas.borrow().clone()
    }

    /// Points the model at this area's group and selection. Nested borrows
    /// find it already done.
    fn apply(&self) {
        let Ok(mut model) = self.shared.try_borrow_mut() else {
            return;
        };
        let view = self.view.borrow();
        if model.path() != view.path.as_slice() {
            let path = model.valid_path(&view.path);
            if model.path() != path.as_slice() {
                model.set_path(path);
                self.bump();
            }
        }
        if model.selected_node_ids() != view.selected.as_slice() {
            model.set_selected_node_ids(view.selected.clone());
            self.bump();
        }
    }

    fn bump(&self) {
        self.revision.set(self.revision.get() + 1);
    }

    pub fn borrow(&self) -> Ref<'_, GraphModel> {
        self.apply();
        self.shared.borrow()
    }

    pub fn borrow_mut(&self) -> ModelMut<'_> {
        self.apply();
        self.bump();
        ModelMut {
            model: self.shared.borrow_mut(),
            view: &self.view,
        }
    }

    pub fn set_path_to_root(&self) {
        let mut view = self.view.borrow_mut();
        view.path.clear();
        view.selected.clear();
    }
}

impl Deref for ModelMut<'_> {
    type Target = GraphModel;

    fn deref(&self) -> &GraphModel {
        &self.model
    }
}

impl DerefMut for ModelMut<'_> {
    fn deref_mut(&mut self) -> &mut GraphModel {
        &mut self.model
    }
}

impl Drop for ModelMut<'_> {
    fn drop(&mut self) {
        let mut view = self.view.borrow_mut();
        view.path = self.model.path().to_vec();
        view.selected = self.model.selected_node_ids().to_vec();
    }
}
