use sha2::{Digest as _, Sha256};

/// A UUID-shaped id derived from `key`, so building twice gives the same files.
pub fn stable_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0u8; 16];
    for (byte, source) in bytes.iter_mut().zip(digest.iter()) {
        *byte = *source;
    }
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// The handle id of the input or output `key` of group `group`.
pub fn handle_id(group: &str, input: bool, key: &str) -> String {
    let side = if input { "in" } else { "out" };
    stable_id(&format!("{group}/{side}/{key}"))
}
