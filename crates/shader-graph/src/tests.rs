use super::*;

fn validate(wgsl: &str) {
    let module = naga::front::wgsl::parse_str(wgsl)
        .unwrap_or_else(|e| panic!("{}\n{wgsl}", e.emit_to_string(wgsl)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}\n{wgsl}"));
}

fn endpoint(node: NodeId, socket: usize) -> Endpoint {
    Endpoint { node, socket }
}

#[test]
fn demo_generates_valid_wgsl() {
    validate(&generate(&Project::demo()).unwrap());
}

#[test]
fn every_node_kind_generates_valid_wgsl() {
    let mut project = Project::default();
    let output = project.root.nodes[0].id;
    let uv = project
        .add_node(DocId::Root, NodeKind::Uv, [0.0; 2])
        .unwrap();
    for kind in NodeKind::catalog() {
        let node = project.add_node(DocId::Root, kind, [0.0; 2]).unwrap();
        if !project.signature(DocId::Root, kind).inputs.is_empty() {
            project
                .connect(
                    DocId::Root,
                    Edge {
                        from: endpoint(uv, 0),
                        to: endpoint(node, 0),
                    },
                )
                .unwrap();
        }
        project
            .connect(
                DocId::Root,
                Edge {
                    from: endpoint(node, 0),
                    to: endpoint(output, 0),
                },
            )
            .unwrap();
        validate(&generate(&project).unwrap());
    }
}

#[test]
fn rejects_cycles() {
    let mut project = Project::default();
    let a = project
        .add_node(DocId::Root, NodeKind::Math(MathOp::Add), [0.0; 2])
        .unwrap();
    let b = project
        .add_node(DocId::Root, NodeKind::Math(MathOp::Sin), [0.0; 2])
        .unwrap();
    project
        .connect(
            DocId::Root,
            Edge {
                from: endpoint(a, 0),
                to: endpoint(b, 0),
            },
        )
        .unwrap();
    let cycle = project.connect(
        DocId::Root,
        Edge {
            from: endpoint(b, 0),
            to: endpoint(a, 0),
        },
    );
    assert_eq!(cycle, Err(EditError::Cycle));
}

#[test]
fn grouping_keeps_the_shader_valid_and_links_through_the_group() {
    let mut project = Project::demo();
    let selected: Vec<NodeId> = project
        .root
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Math(_)))
        .map(|n| n.id)
        .collect();
    let group_node = project.group_nodes(DocId::Root, &selected).unwrap();
    let NodeKind::Group(group) = project.root.node(group_node).unwrap().kind else {
        panic!()
    };
    let interface = &project.groups[&group].interface;
    assert_eq!(interface.inputs, ["u", "seconds"]);
    assert_eq!(interface.outputs, ["result"]);

    let wgsl = generate(&project).unwrap();
    assert!(wgsl.contains(&format!("fn group_{}(i0: f32, i1: f32)", group.0)));
    validate(&wgsl);

    // A group inside another group.
    let outer = project.group_nodes(DocId::Root, &[group_node]).unwrap();
    validate(&generate(&project).unwrap());
    let NodeKind::Group(outer) = project.root.node(outer).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        project.add_node(DocId::Group(group), NodeKind::Group(outer), [0.0; 2]),
        Err(EditError::RecursiveGroup)
    );
}
