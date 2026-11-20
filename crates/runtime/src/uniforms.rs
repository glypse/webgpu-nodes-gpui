use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    F32,
    U32,
    I32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UniformField {
    /// Byte offset in the block.
    pub offset: u32,
    pub scalar: Scalar,
    pub components: u8,
}

/// A uniform buffer bound at `@group(0) @binding(binding)`, laid out by WGSL's rules.
#[derive(Clone, Debug, PartialEq)]
pub struct UniformBlock {
    pub binding: u32,
    /// Byte size, a multiple of 16.
    pub size: u32,
    pub fields: BTreeMap<String, UniformField>,
}

/// Writes `values` into `target` at the offsets `block` lays out. Fields
/// without a value, and values for fields the block lacks, are left alone;
/// a value shorter than its field pads with zeros.
pub fn pack_block(block: &UniformBlock, values: &BTreeMap<String, Vec<f64>>, target: &mut [u8]) {
    for (name, field) in &block.fields {
        let Some(value) = values.get(name) else {
            continue;
        };
        for i in 0..usize::from(field.components) {
            let at = field.offset as usize + i * 4;
            let Some(slot) = target.get_mut(at..at + 4) else {
                continue;
            };
            let v = value.get(i).copied().unwrap_or(0.0);
            let bytes = match field.scalar {
                Scalar::F32 => (v as f32).to_le_bytes(),
                Scalar::U32 => (v as u32).to_le_bytes(),
                Scalar::I32 => (v as i32).to_le_bytes(),
            };
            slot.copy_from_slice(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_fields_at_their_offsets() {
        let block = UniformBlock {
            binding: 0,
            size: 32,
            fields: BTreeMap::from([
                (
                    "a".into(),
                    UniformField {
                        offset: 0,
                        scalar: Scalar::F32,
                        components: 1,
                    },
                ),
                (
                    "b".into(),
                    UniformField {
                        offset: 16,
                        scalar: Scalar::U32,
                        components: 2,
                    },
                ),
            ]),
        };
        let values = BTreeMap::from([("a".to_string(), vec![1.5]), ("b".to_string(), vec![7.0])]);
        let mut bytes = [0u8; 32];
        pack_block(&block, &values, &mut bytes);
        assert_eq!(&bytes[0..4], &1.5f32.to_le_bytes());
        assert_eq!(&bytes[16..20], &7u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &0u32.to_le_bytes());
    }
}
