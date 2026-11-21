//! The benchmark scene `scenes/voronoi.json`, built through the graph model so it
//! stays valid as the model changes. `WRITE_SCENES=1 cargo test -p webgpu-nodes`
//! rewrites the file; a plain test run fails when it is out of date.

#[cfg(test)]
mod tests {
    use shader_graph::graph::{Connection, FRAG_OUTPUT, GraphModel, GraphProject, Point};
    use shader_graph::library;
    use shader_graph::types::{InputValue, WgslType};

    const VORONOI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes/voronoi.json");
    const DEFAULT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes/default.json");

    fn library_ref(id: &str) -> String {
        library::entries()
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("the library has {id}"))
            .reference
            .clone()
    }

    /// The id of the port named `name` (by id or label) on a node, or a panic listing the ports.
    fn port(model: &GraphModel, node: &str, input: bool, name: &str) -> String {
        let ports: Vec<(String, String)> = if input {
            model.resolve_node_inputs(node).into_iter().map(|p| (p.id, p.label)).collect()
        } else {
            model.resolve_node_outputs(node).into_iter().map(|p| (p.id, p.label)).collect()
        };
        ports
            .iter()
            .find(|(id, label)| id == name || label == name)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("node {node} has no port {name}; it has {ports:?}"))
    }

    /// Links `from.1` of node `from.0` to `to.1` of node `to.0`, by port id or label.
    fn wire(model: &mut GraphModel, from: (&str, &str), to: (&str, &str)) {
        let source = port(model, from.0, false, from.1);
        let target = port(model, to.0, true, to.1);
        let connection = Connection::new(from.0, source, to.0, target);
        assert!(model.try_connect(connection.clone()), "links {connection:?}");
    }

    fn set(model: &mut GraphModel, node: &str, input: &str, value: f64) {
        let id = port(model, node, true, input);
        model.set_input_value(node, &id, Some(InputValue::from(value)));
    }

    fn add(model: &mut GraphModel, node_type: &str) -> String {
        model.add_node(node_type, Point::default()).unwrap_or_else(|| panic!("adds {node_type}"))
    }

    fn add_library(model: &mut GraphModel, id: &str) -> String {
        model
            .add_group_instance(&library_ref(id), Point::default())
            .unwrap_or_else(|| panic!("adds {id}"))
    }

    /// UV, aspect-corrected, split into x and y; z is half of time. The vec3 drives
    /// a fractal Voronoi, remapped and shown as OKLCh.
    fn voronoi() -> GraphModel {
        let mut model = GraphModel::default();
        let output = model
            .root()
            .nodes
            .iter()
            .find(|n| n.is(FRAG_OUTPUT))
            .expect("the root has an output")
            .id
            .clone();
        let uv = add(&mut model, "uv");
        let aspect = add_library(&mut model, "std-aspect-uv");
        let separate = add(&mut model, "separate-vector");
        let time = add(&mut model, "time");
        let half = add(&mut model, "div");
        set(&mut model, &half, "b", 2.0);
        let declare = add(&mut model, "declare");
        model.set_node_type(&declare, Some(WgslType::Vec3f));
        let voronoi = add_library(&mut model, "std-fractal-voronoi");
        let remap = add_library(&mut model, "std-remap");
        let color = add_library(&mut model, "std-oklch-to-linear-rgb");

        wire(&mut model, (&uv, "uv"), (&aspect, "Coordinates"));
        wire(&mut model, (&aspect, "Coordinates"), (&separate, "v"));
        wire(&mut model, (&time, "seconds"), (&half, "a"));
        wire(&mut model, (&separate, "x"), (&declare, "x"));
        wire(&mut model, (&separate, "y"), (&declare, "y"));
        wire(&mut model, (&half, "result"), (&declare, "z"));
        wire(&mut model, (&declare, "value"), (&voronoi, "Vector"));
        wire(&mut model, (&voronoi, "Distance"), (&remap, "Value"));
        set(&mut model, &remap, "To Min", 0.3);
        set(&mut model, &remap, "To Max", 0.9);
        let lch = add(&mut model, "declare");
        model.set_node_type(&lch, Some(WgslType::Vec3f));
        set(&mut model, &lch, "y", 0.15);
        wire(&mut model, (&remap, "Result"), (&lch, "x"));
        wire(&mut model, (&half, "result"), (&lch, "z"));
        wire(&mut model, (&lch, "value"), (&color, "OKLch"));
        wire(&mut model, (&color, "Linear RGB"), (&output, "color"));
        model
    }

    /// Panics unless the model's program generates WGSL that naga accepts.
    fn assert_valid(model: &GraphModel) {
        let program = model.generate().expect("the scene generates");
        let code = program.program.code;
        let module = naga::front::wgsl::parse_str(&code)
            .unwrap_or_else(|e| panic!("{}\n{code}", e.emit_to_string(&code)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{e:?}\n{code}"));
    }

    #[test]
    fn voronoi_scene_compiles_to_valid_wgsl() {
        assert_valid(&voronoi());
    }

    #[test]
    fn voronoi_scene_file_loads_and_compiles() {
        let text = std::fs::read_to_string(VORONOI).expect("scenes/voronoi.json exists");
        let project = GraphProject::from_json(&text).expect("the scene loads");
        assert_valid(&GraphModel::new(project));
    }

    /// Rewrites `scenes/voronoi.json` from the builder:
    /// `cargo test -p webgpu-nodes -- --ignored write_voronoi_scene`.
    #[test]
    #[ignore]
    fn write_voronoi_scene() {
        std::fs::write(VORONOI, format!("{}\n", voronoi().project().to_json())).expect("writes the scene");
    }

    #[test]
    fn default_scene_loads_and_compiles() {
        let text = std::fs::read_to_string(DEFAULT).expect("scenes/default.json exists");
        let project = GraphProject::from_json(&text).expect("the scene loads");
        GraphModel::new(project).generate().expect("the scene generates");
    }
}
