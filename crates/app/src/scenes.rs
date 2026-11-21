//! The benchmark scenes in `scenes/` must keep loading and compiling as the model changes.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use shader_graph::graph::GraphModel;

    fn assert_compiles(file: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes").join(file);
        let loaded = crate::project::read(&path).unwrap_or_else(|e| panic!("{file}: {e:#}"));
        let program = GraphModel::new(loaded.project).generate().unwrap_or_else(|e| panic!("{file}: {e}"));
        let code = program.program.code;
        let module = naga::front::wgsl::parse_str(&code)
            .unwrap_or_else(|e| panic!("{file}: {}\n{code}", e.emit_to_string(&code)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }

    #[test]
    fn default_scene_compiles() {
        assert_compiles("default.json");
    }

    #[test]
    fn voronoi_scene_compiles() {
        assert_compiles("voronoi.wgnodes");
    }
}
