//! What a saved file may ask for. The limits are far above anything drawn by
//! hand, yet low enough that the work a file can cause stays bounded.

/// The most cases an Index Switch may have; a count of cases is also what codegen loops over.
pub const MAX_CASES: u32 = 1024;
/// The most a loop counter can reach: WGSL's `i32`.
pub const MAX_ITERATIONS: u32 = i32::MAX as u32;
pub const MAX_DEFINITIONS: usize = 2000;
pub const MAX_NODES_PER_DOCUMENT: usize = 20_000;
pub const MAX_EDGES_PER_DOCUMENT: usize = 80_000;
pub const MAX_PROJECT_NODES: usize = 100_000;
/// Links in a row, which type inference and codegen follow recursively.
pub const MAX_CHAIN_LENGTH: usize = 2000;
/// Groups nested in a row, for the same reason.
pub const MAX_GROUP_DEPTH: usize = 64;
/// Sockets, zone items, curve points, type variables and the like, on one node or document.
pub const MAX_LIST_LENGTH: usize = 1024;
pub const MAX_ASSETS: usize = 4096;
pub const MAX_ID_LENGTH: usize = 256;
pub const MAX_LABEL_LENGTH: usize = 1024;
pub const MAX_NOTE_LENGTH: usize = 64 * 1024;
/// Longest Custom WGSL body, in bytes.
pub const MAX_CODE_LENGTH: usize = 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
/// Bundles hold their media whole, so unlike the rest this is about what fits in memory.
pub const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_ASSET_SIDE: u32 = 65536;
