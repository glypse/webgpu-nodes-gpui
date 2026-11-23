//! Building every described group: dependencies first, each with the
//! version its definition has.

use std::cell::RefCell;

use shader_graph::graph::GraphDocument;
use shader_graph::graph::GroupWidget;

use crate::builder::{
    BuildError, BuildErrors, ErrorKind, GroupBuilder, GroupSpec, InputIds, ResolvedGroup,
};
use crate::json;

/// A described group, built.
#[derive(Clone, Debug)]
pub struct BuiltGroup {
    pub spec: GroupSpec,
    /// Its nodes are unplaced until laid out.
    pub definition: GraphDocument,
    pub widget: Option<GroupWidget>,
    /// The refs of the groups it calls.
    pub dependencies: Vec<String>,
    /// What group nodes reference: the id and version.
    pub reference: String,
    /// Input keys and the handle ids they have.
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

impl BuiltGroup {
    fn resolved(&self) -> ResolvedGroup {
        ResolvedGroup {
            reference: self.reference.clone(),
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
        }
    }

    /// The version of its definition, as the library derives it from the file.
    pub fn version(&self) -> String {
        shader_graph::library::version_of(&json::definition_value(&self.definition))
    }
}

struct Builder<'a> {
    specs: &'a [GroupSpec],
    built: RefCell<Vec<BuiltGroup>>,
    failed: RefCell<Vec<&'a str>>,
    stack: RefCell<Vec<&'a str>>,
    errors: RefCell<Vec<BuildError>>,
}

impl<'a> Builder<'a> {
    fn resolve(&self, id: &str) -> Result<ResolvedGroup, ErrorKind> {
        if let Some(group) = self.built.borrow().iter().find(|g| g.spec.id == id) {
            return Ok(group.resolved());
        }
        let spec = self
            .specs
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| ErrorKind::NoGroup(id.to_owned()))?;
        if self.stack.borrow().contains(&spec.id) {
            return Err(ErrorKind::Cycle(id.to_owned()));
        }
        if self.failed.borrow().contains(&spec.id) {
            return Err(ErrorKind::DependencyFailed(id.to_owned()));
        }
        self.build(spec)
    }

    fn build(&self, spec: &'a GroupSpec) -> Result<ResolvedGroup, ErrorKind> {
        self.stack.borrow_mut().push(spec.id);
        let resolver = |id: &str| self.resolve(id);
        let run = |salt: &str| {
            let g = GroupBuilder::new(spec.id, salt, &resolver);
            (spec.build)(&g);
            g.finish(spec.inputs, spec.outputs)
        };
        // Ids salted with what the group holds: any change to it renews them all.
        let result = run("").and_then(|unsalted| {
            let fingerprint =
                shader_graph::library::version_of(&json::definition_value(&unsalted.document));
            run(&fingerprint)
        });
        self.stack.borrow_mut().pop();
        let finished = match result {
            Ok(finished) => finished,
            Err(kinds) => return Err(self.fail(spec, kinds)),
        };
        let ids = InputIds {
            keys: &finished.input_keys,
            missing: RefCell::new(Vec::new()),
        };
        let widget = spec.widget.map(|widget| widget(&ids));
        let missing = ids.missing.into_inner();
        if !missing.is_empty() {
            return Err(self.fail(
                spec,
                missing.into_iter().map(ErrorKind::WidgetInput).collect(),
            ));
        }
        let mut group = BuiltGroup {
            spec: spec.clone(),
            definition: finished.document,
            widget,
            dependencies: finished.dependencies,
            reference: String::new(),
            inputs: finished.input_keys,
            outputs: finished.output_keys,
        };
        group.reference = shader_graph::library::make_ref(spec.id, &group.version());
        let resolved = group.resolved();
        self.built.borrow_mut().push(group);
        Ok(resolved)
    }

    fn fail(&self, spec: &'a GroupSpec, kinds: Vec<ErrorKind>) -> ErrorKind {
        self.failed.borrow_mut().push(spec.id);
        self.errors
            .borrow_mut()
            .extend(kinds.into_iter().map(|kind| BuildError {
                group: spec.id.to_owned(),
                kind,
            }));
        ErrorKind::DependencyFailed(spec.id.to_owned())
    }
}

/// Build `specs` and the groups they call, dependencies first.
pub fn build_groups(specs: &[GroupSpec]) -> Result<Vec<BuiltGroup>, BuildErrors> {
    let builder = Builder {
        specs,
        built: RefCell::new(Vec::new()),
        failed: RefCell::new(Vec::new()),
        stack: RefCell::new(Vec::new()),
        errors: RefCell::new(Vec::new()),
    };
    for spec in specs {
        let done = builder.built.borrow().iter().any(|g| g.spec.id == spec.id)
            || builder.failed.borrow().contains(&spec.id);
        if !done && let Err(kind) = builder.build(spec) {
            // Errors of a failed group are already recorded; a group calling an unknown or cyclic one reports it.
            if !matches!(kind, ErrorKind::DependencyFailed(_)) {
                builder.errors.borrow_mut().push(BuildError {
                    group: spec.id.to_owned(),
                    kind,
                });
            }
        }
    }
    let errors = builder.errors.into_inner();
    if errors.is_empty() {
        Ok(builder.built.into_inner())
    } else {
        Err(BuildErrors(errors))
    }
}
