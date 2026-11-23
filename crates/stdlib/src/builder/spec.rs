use std::cell::RefCell;

use shader_graph::graph::GroupWidget;

use super::group::GroupBuilder;

/// The handle id of each input of a group, by key.
pub struct InputIds<'a> {
    pub(crate) keys: &'a [(String, String)],
    pub(crate) missing: RefCell<Vec<String>>,
}

impl InputIds<'_> {
    /// The handle id of the input `key`; an unknown key is reported when the group is built.
    pub fn id(&self, key: &str) -> String {
        match self.keys.iter().find(|(k, _)| k == key) {
            Some((_, id)) => id.clone(),
            None => {
                self.missing.borrow_mut().push(key.to_owned());
                String::new()
            }
        }
    }
}

/// A group described in Rust. `inputs` and `outputs` are the keys `build`
/// gives its inputs and outputs (in any order): they're what a group calling
/// it may feed and read.
#[derive(Clone, Debug)]
pub struct GroupSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub category: &'static str,
    /// Sections of the add menu it's in, under its category.
    pub section: Vec<&'static str>,
    /// The section of search results it's listed in, after the main results.
    pub search_section: Option<&'static str>,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
    pub build: fn(&GroupBuilder<'_>),
    /// The widget its nodes draw, given the handle id of each input by key.
    pub widget: Option<fn(&InputIds<'_>) -> GroupWidget>,
}

impl GroupSpec {
    pub fn new(
        id: &'static str,
        name: &'static str,
        category: &'static str,
        inputs: &'static [&'static str],
        outputs: &'static [&'static str],
        build: fn(&GroupBuilder<'_>),
    ) -> Self {
        Self {
            id,
            name,
            aliases: &[],
            category,
            section: Vec::new(),
            search_section: None,
            inputs,
            outputs,
            build,
            widget: None,
        }
    }

    pub fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    /// Listed in the Helpers section of its category (under `path`), and after the main search results.
    pub fn helper(mut self, path: &[&'static str]) -> Self {
        self.section = std::iter::once("Helpers")
            .chain(path.iter().copied())
            .collect();
        self.search_section = Some("Helpers");
        self
    }

    pub fn widget(mut self, widget: fn(&InputIds<'_>) -> GroupWidget) -> Self {
        self.widget = Some(widget);
        self
    }
}
