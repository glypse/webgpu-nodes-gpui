//! Compiling every group the way a node of it is used, and checking the WGSL.

use std::collections::BTreeMap;

use shader_graph::graph::{FRAG_OUTPUT, GraphDocument, GraphModel, GraphNode, NODE_GROUP, Point};
use shader_graph::types::{InputValue, WgslType};

use crate::build::BuiltGroup;
use crate::layout::{alias, project_of};

/// More combinations than this of a group's enum values and types are left unchecked.
const MAX_COMBINATIONS: usize = 4096;

/// Every combination of the values of `choices`, by key.
fn combinations<T: Clone>(choices: &[(String, Vec<T>)]) -> Vec<BTreeMap<String, T>> {
    let mut result = vec![BTreeMap::new()];
    for (key, values) in choices {
        result = result
            .into_iter()
            .flat_map(|combo| {
                values.iter().map(move |value| {
                    let mut next: BTreeMap<String, T> = combo.clone();
                    next.insert(key.clone(), value.clone());
                    next
                })
            })
            .collect();
    }
    result
}

/// naga's verdict on `code`.
fn check_wgsl(code: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(code).map_err(|e| e.emit_to_string(code))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map(drop)
    .map_err(|e| format!("{e:?}"))
}

/// The root of a project using the group `group`: one node of it, with
/// `values` and `types` set, its outputs summed into the fragment output.
fn root_using(
    group: &BuiltGroup,
    outputs: &[&str],
    values: &BTreeMap<String, i64>,
    types: &BTreeMap<String, WgslType>,
) -> GraphDocument {
    let mut root = GraphDocument {
        nodes: vec![GraphNode::new(FRAG_OUTPUT, Point::default())],
        ..Default::default()
    };
    let out = root.nodes[0].id.clone();
    let mut instance = GraphNode::new(NODE_GROUP, Point::default());
    instance.group = Some(shader_graph::graph::GroupRef {
        definition_ref: alias(group),
        locked: true,
    });
    instance.data.values.extend(
        values
            .iter()
            .map(|(k, v)| (k.clone(), InputValue::from(*v))),
    );
    instance
        .data
        .types
        .extend(types.iter().map(|(k, v)| (k.clone(), *v)));
    let instance_id = instance.id.clone();
    root.nodes.push(instance);
    let target = if let [only] = outputs {
        let edge = shader_graph::graph::Connection::new(&instance_id, *only, &out, "color");
        root.edges.push(edge.into_edge());
        return root;
    } else {
        let mut sum = GraphNode::new("add", Point::default());
        sum.id = "sum".to_owned();
        root.nodes.push(sum);
        "sum"
    };
    for handle in outputs {
        let edge = shader_graph::graph::Connection::new(&instance_id, *handle, target, "operands");
        root.edges.push(edge.into_edge());
    }
    root.edges
        .push(shader_graph::graph::Connection::new(target, "result", &out, "color").into_edge());
    root
}

/// Compile a node of each of `checked`, with every combination of its enum
/// values and its variables' types, and validate the WGSL. `groups` are all
/// the groups built, which the checked ones may call. Returns the problems.
pub fn validate_groups(groups: &[BuiltGroup], checked: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    for group in groups.iter().filter(|g| checked.contains(&g.spec.id)) {
        let outputs: Vec<&str> = group.outputs.iter().map(|(_, h)| h.as_str()).collect();
        let enums: Vec<(String, Vec<i64>)> = group
            .definition
            .inputs
            .iter()
            .filter_map(|h| {
                let variants = h.variants.as_ref()?;
                Some((h.id.clone(), variants.iter().map(|v| v.value).collect()))
            })
            .collect();
        let variables: Vec<(String, Vec<WgslType>)> = group
            .definition
            .type_variables
            .iter()
            .map(|v| (v.id.clone(), v.types.clone()))
            .collect();
        let (value_combos, type_combos) = (combinations(&enums), combinations(&variables));
        if value_combos.len() * type_combos.len() > MAX_COMBINATIONS {
            problems.push(format!(
                "{}: too many combinations of enum values and types to check",
                group.spec.id
            ));
            continue;
        }
        for values in &value_combos {
            for types in &type_combos {
                let root = root_using(group, &outputs, values, types);
                let model = GraphModel::new(project_of(groups, Some(root)));
                let context = format!(
                    "{}, enums {:?}, types {:?}",
                    group.spec.id,
                    values.values().collect::<Vec<_>>(),
                    types.values().map(|t| t.id()).collect::<Vec<_>>()
                );
                match model.generate() {
                    Ok(program) => {
                        for pass in &program.plan.passes {
                            if let Err(e) = check_wgsl(&pass.program.code) {
                                problems.push(format!("{context}: {e}"));
                            }
                        }
                    }
                    Err(e) => problems.push(format!("{context}: {e}")),
                }
            }
        }
    }
    problems
}
