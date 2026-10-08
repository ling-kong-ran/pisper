//! Port of `packages/ai/src/utils/uuid.ts` (48 lines): the time-ordered
//! UUIDv7 generator the provider APIs and harness summary requests use for
//! session/request ids.
//!
//! Disclosed substitution: the port has no WebCrypto, so the random bytes
//! come from per-call `RandomState` hashes seeded with the clock (the same
//! disclosed pseudo-randomness the codex port used before this module was
//! shared, and the retry jitter). The monotonic-sequence state is
//! process-global like the upstream module-level `sequence`, so every
//! caller (codex websocket ids, summary session ids) shares one ordering.

use std::sync::Mutex;

const MAX_SEQUENCE: u64 = (1u64 << 41) - 1;

struct UuidV7State {
    last_ordinary_timestamp: i64,
    sequence: Option<u64>,
}

static UUID_STATE: Mutex<UuidV7State> = Mutex::new(UuidV7State {
    last_ordinary_timestamp: -1,
    sequence: None,
});

/// Process-random bytes (upstream `crypto.getRandomValues`).
fn random_bytes() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (index, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(crate::ai::now_ms() as u64 ^ (index as u64) << 32);
        let value = hasher.finish();
        chunk.copy_from_slice(&value.to_ne_bytes()[..chunk.len()]);
    }
    bytes
}

/// Upstream `uuidv7()` (`uuid.ts:15-24` no-timestamp branch): time-ordered
/// with a 41-bit monotonic sequence.
pub fn uuid_v7() -> String {
    let mut bytes = random_bytes();
    let mut state = UUID_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let timestamp = crate::ai::now_ms().max(state.last_ordinary_timestamp);
    state.last_ordinary_timestamp = timestamp;
    state.sequence = match state.sequence {
        None => Some(
            ((bytes[1] as u64) << 32)
                | ((bytes[2] as u64) << 24)
                | ((bytes[3] as u64) << 16)
                | ((bytes[4] as u64) << 8)
                | (bytes[5] as u64),
        ),
        // The 41-bit sequence is effectively inexhaustible at one UUID per
        // millisecond; upstream throws here.
        Some(sequence) if sequence < MAX_SEQUENCE => Some(sequence + 1),
        Some(sequence) => Some(sequence),
    };
    let sequence = state.sequence.unwrap_or(0);
    drop(state);
    assemble_uuid_v7(&mut bytes, timestamp as u64, sequence)
}

/// Upstream `uuidv7(timestampMs)` (`uuid.ts:15-24` supplied-timestamp
/// branch): the requested timestamp is preserved exactly (no monotonic clamp,
/// `lastOrdinaryTimestamp` untouched) while the shared sequence still
/// advances, so follower ids at the same timestamp stay distinct. Ported for
/// the session layer's id minting (`session.ts`, `legacy-v3.ts` reminting).
/// `Err` is the upstream `RangeError`.
pub fn uuid_v7_at(timestamp_ms: i64) -> anyhow::Result<String> {
    const MAX_UUID_V7_TIMESTAMP: i64 = 0xffff_ffff_ffff;
    if !(0..=MAX_UUID_V7_TIMESTAMP).contains(&timestamp_ms) {
        anyhow::bail!("UUIDv7 timestamp must be an integer between 0 and {MAX_UUID_V7_TIMESTAMP}");
    }
    let mut bytes = random_bytes();
    let mut state = UUID_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.sequence = match state.sequence {
        None => Some(
            ((bytes[1] as u64) << 32)
                | ((bytes[2] as u64) << 24)
                | ((bytes[3] as u64) << 16)
                | ((bytes[4] as u64) << 8)
                | (bytes[5] as u64),
        ),
        Some(sequence) if sequence < MAX_SEQUENCE => Some(sequence + 1),
        // Upstream throws "UUIDv7 generator sequence exhausted".
        Some(_) => anyhow::bail!("UUIDv7 generator sequence exhausted"),
    };
    let sequence = state.sequence.unwrap_or(0);
    drop(state);
    Ok(assemble_uuid_v7(&mut bytes, timestamp_ms as u64, sequence))
}

/// The shared byte assembly of both upstream branches (`uuid.ts:32-49`):
/// 48-bit big-endian timestamp, version 7 nibble, RFC variant bits, and the
/// 41-bit sequence spread over bytes 6-11.
fn assemble_uuid_v7(bytes: &mut [u8; 16], timestamp: u64, sequence: u64) -> String {
    for (index, shift) in (0..6).rev().enumerate() {
        bytes[index] = ((timestamp >> (shift * 8)) & 0xff) as u8;
    }
    bytes[6] = 0x70 | ((sequence >> 37) & 0x0f) as u8;
    bytes[7] = ((sequence >> 29) & 0xff) as u8;
    bytes[8] = 0x80 | ((sequence >> 23) & 0x3f) as u8;
    bytes[9] = ((sequence >> 15) & 0xff) as u8;
    bytes[10] = ((sequence >> 7) & 0xff) as u8;
    bytes[11] = ((((sequence & 0x7f) << 1) | ((bytes[11] as u64) & 0x01)) & 0xff) as u8;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v7_shape_and_uniqueness() {
        let first = uuid_v7();
        let second = uuid_v7();
        assert_ne!(first, second);
        let bytes = [first, second];
        for id in bytes {
            assert_eq!(id.len(), 36, "uuid shape: {id}");
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(
                parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12]
            );
            assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            // Version 7 nibble and the RFC variant bits.
            assert_eq!(parts[2].chars().next().unwrap(), '7');
            assert!(matches!(
                parts[3].chars().next().unwrap(),
                '8' | '9' | 'a' | 'b'
            ));
        }
    }
}
