use std::collections::BTreeMap;

use super::literals::UniformValue;

/// What the generated code reads per draw: the frame's timing and the target's size and gamut.
pub const FRAME_VAR: &str = "runtime_frame";
/// The frame field telling whether the target is Display P3 (1) rather than sRGB (0).
pub const DISPLAY_P3_FIELD: &str = "display_p3";
/// The struct holding the values of uniform inputs.
pub const UNIFORMS_VAR: &str = "runtime_uniforms";
/// Per-fragment values readable from any function, unlike `frag()`'s own parameters.
pub const FRAGMENT_VAR: &str = "runtime_fragment";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniformScalar {
    F32,
    U32,
    I32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniformField {
    /// Byte offset in the block.
    pub offset: u32,
    pub scalar: UniformScalar,
    pub components: u32,
}

/// A uniform buffer bound at `@group(0) @binding(binding)`, laid out by WGSL's rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniformBlock {
    pub binding: u32,
    /// Byte size, a multiple of 16.
    pub size: u32,
    /// Fields in declaration order, by name.
    pub fields: BTreeMap<String, UniformField>,
}

/// A complete WGSL module drawing the graph over a fullscreen triangle, and
/// the buffers it reads. The fragment entry returns a linear, straight-alpha
/// colour.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderProgram {
    pub code: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    /// The frame block, written by the runtime on each draw: fields named
    /// after `FRAME_FIELDS`.
    pub frame: UniformBlock,
    /// Fields named after the uniforms the shader reads.
    pub uniforms: UniformBlock,
}

/// A frame field: its name, scalar type and component count.
#[derive(Clone, Copy, Debug)]
pub struct FieldDecl {
    pub name: &'static str,
    pub scalar: UniformScalar,
    pub components: u32,
}

pub const FRAME_FIELDS: [FieldDecl; 5] = [
    FieldDecl { name: "time", scalar: UniformScalar::F32, components: 1 },
    FieldDecl { name: "delta", scalar: UniformScalar::F32, components: 1 },
    FieldDecl { name: "resolution", scalar: UniformScalar::F32, components: 2 },
    FieldDecl { name: "frame", scalar: UniformScalar::U32, components: 1 },
    FieldDecl { name: DISPLAY_P3_FIELD, scalar: UniformScalar::U32, components: 1 },
];

struct Decl {
    name: String,
    scalar: UniformScalar,
    components: u32,
}

fn wgsl_type(d: &Decl) -> String {
    if d.components == 1 {
        return scalar_name(d.scalar).to_owned();
    }
    let suffix = match d.scalar {
        UniformScalar::F32 => 'f',
        UniformScalar::U32 => 'u',
        UniformScalar::I32 => 'i',
    };
    format!("vec{}{suffix}", d.components)
}

fn scalar_name(s: UniformScalar) -> &'static str {
    match s {
        UniformScalar::F32 => "f32",
        UniformScalar::U32 => "u32",
        UniformScalar::I32 => "i32",
    }
}

/// Offsets per WGSL's uniform layout: vec2 aligns to 8 bytes, vec3 and vec4 to 16.
fn layout(binding: u32, decls: &[Decl]) -> UniformBlock {
    let mut fields = BTreeMap::new();
    let mut offset = 0u32;
    for d in decls {
        let align = match d.components {
            1 => 4,
            2 => 8,
            _ => 16,
        };
        offset = offset.div_ceil(align) * align;
        fields.insert(
            d.name.clone(),
            UniformField { offset, scalar: d.scalar, components: d.components },
        );
        offset += d.components * 4;
    }
    UniformBlock { binding, size: (offset.div_ceil(16) * 16).max(16), fields }
}

fn struct_source(name: &str, decls: &[Decl]) -> String {
    let fields: Vec<String> =
        decls.iter().map(|d| format!("\t{}: {},", d.name, wgsl_type(d))).collect();
    format!("struct {name} {{\n{}\n}};", fields.join("\n"))
}

/// Wrap a fragment from `generate_shader` (which defines `frag`) into a full program.
pub fn build_program(fragment: &str, uniforms: &[(String, UniformValue)]) -> ShaderProgram {
    let mut uniform_decls: Vec<Decl> = uniforms
        .iter()
        .map(|(name, value)| Decl {
            name: name.clone(),
            scalar: UniformScalar::F32,
            components: value.components() as u32,
        })
        .collect();
    // A struct can't be empty: this stands in when the graph has no uniforms.
    if uniform_decls.is_empty() {
        uniform_decls.push(Decl { name: "unused".to_owned(), scalar: UniformScalar::F32, components: 4 });
    }
    let frame_decls: Vec<Decl> = FRAME_FIELDS
        .iter()
        .map(|f| Decl { name: f.name.to_owned(), scalar: f.scalar, components: f.components })
        .collect();
    let frame = layout(0, &frame_decls);
    let uniform_block = layout(1, &uniform_decls);
    // uv's origin is the bottom-left corner, y pointing up.
    let code = format!(
        "{frame_struct}
{uniform_struct}
struct RuntimeFragment {{
	uv: vec2f,
	position: vec2f,
}};
@group(0) @binding({fb}) var<uniform> {FRAME_VAR}: RuntimeFrame;
@group(0) @binding({ub}) var<uniform> {UNIFORMS_VAR}: RuntimeUniforms;
var<private> {FRAGMENT_VAR}: RuntimeFragment;

{fragment}

struct RuntimeVarying {{
	@builtin(position) position: vec4f,
	@location(0) uv: vec2f,
}};

@vertex
fn runtime_vertex(@builtin(vertex_index) index: u32) -> RuntimeVarying {{
	let corner = vec2f(f32(index & 1u) * 4.0 - 1.0, f32(index >> 1u) * 4.0 - 1.0);
	return RuntimeVarying(vec4f(corner, 0.0, 1.0), (corner + 1.0) * 0.5);
}}

@fragment
fn runtime_fragment_main(in: RuntimeVarying) -> @location(0) vec4f {{
	{FRAGMENT_VAR}.uv = in.uv;
	{FRAGMENT_VAR}.position = in.position.xy;
	return frag(in.uv);
}}
",
        frame_struct = struct_source("RuntimeFrame", &frame_decls),
        uniform_struct = struct_source("RuntimeUniforms", &uniform_decls),
        fb = frame.binding,
        ub = uniform_block.binding,
    );
    ShaderProgram {
        code,
        vertex_entry: "runtime_vertex".to_owned(),
        fragment_entry: "runtime_fragment_main".to_owned(),
        frame,
        uniforms: uniform_block,
    }
}
