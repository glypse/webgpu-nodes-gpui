//! Scalar and vector arithmetic helpers.

use shader_graph::types::WgslType;

use crate::builder::{GroupSpec, Input, Output};

pub const REMAP: &str = "std-remap";

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            REMAP,
            "Remap",
            "math",
            &["value", "from_min", "from_max", "to_min", "to_max"],
            &["result"],
            |g| {
                let t = g.type_variable(
                    "t",
                    "T",
                    &[
                        WgslType::F32,
                        WgslType::Vec2f,
                        WgslType::Vec3f,
                        WgslType::Vec4f,
                    ],
                );
                let value = g.input("value", Input::new(&t).label("Value").default(0.0));
                let from_min = g.input("from_min", Input::new(&t).label("From Min").default(0.0));
                let from_max = g.input("from_max", Input::new(&t).label("From Max").default(1.0));
                let to_min = g.input("to_min", Input::new(&t).label("To Min").default(0.0));
                let to_max = g.input("to_max", Input::new(&t).label("To Max").default(1.0));
                let to_span = g.op("sub", (to_max, to_min));
                let from_span = g.op("sub", (from_max, from_min));
                let scale = g.op("div", (to_span, from_span));
                let offset = g.op("sub", (value, from_min));
                let scaled = g.op("mul", (offset, scale));
                let result = g.op("add", (to_min, scaled));
                g.output("result", result, Output::new(&t).label("Result"));
            },
        )
        .aliases(&[
            "Map Range",
            "Fit",
            "Range",
            "Rescale",
            "Remap Value Range",
            "Linear",
        ]),
    ]
}
