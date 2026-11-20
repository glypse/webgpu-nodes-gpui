use std::collections::BTreeMap;

use runtime::{ProgramSource, Scalar, UniformBlock, UniformField};

/// The layout of the frame block the generated programs declare, and their entry points.
pub fn program_source(code: String) -> ProgramSource {
    let field = |offset, components| UniformField {
        offset,
        scalar: Scalar::F32,
        components,
    };
    ProgramSource {
        code,
        vertex_entry: "vs_main".into(),
        fragment_entry: "fs_main".into(),
        frame: UniformBlock {
            binding: 0,
            size: 16,
            fields: BTreeMap::from([
                ("time".into(), field(0, 1)),
                ("resolution".into(), field(8, 2)),
            ]),
        },
        uniforms: UniformBlock {
            binding: 1,
            size: 0,
            fields: BTreeMap::new(),
        },
        uniform_values: BTreeMap::new(),
    }
}
