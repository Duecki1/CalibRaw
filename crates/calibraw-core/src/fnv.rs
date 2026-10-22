//! FNV-1a 64-bit hashing, the fingerprint behind cache file names and diagnostics.
//! Offset basis 14695981039346656037, prime 1099511628211 (`0x100000001b3`).

pub(crate) const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Folds one byte into `hash`.
pub(crate) fn step(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(PRIME)
}
