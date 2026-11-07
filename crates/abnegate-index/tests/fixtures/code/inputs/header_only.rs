use std::collections::BTreeMap;

/// Compute a stable fingerprint for a map of settings.
pub fn settings_fingerprint(settings: &BTreeMap<String, String>, salt: u64) -> u64 {
    let mut hash = salt ^ 0xcbf29ce484222325;
    for (key, value) in settings {
        for byte in key.bytes().chain(value.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}
