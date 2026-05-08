//! Identifier types: [`ModelId`] and [`KvKey`].

use std::ops::Range;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Opaque, lightweight model identifier. Shape is `vendor/model[@version]`
/// by convention but the type is opaque — backends interpret it.
///
/// Empty / whitespace-only strings are rejected at construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelId(Arc<str>);

impl Serialize for ModelId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_new(s).map_err(serde::de::Error::custom)
    }
}

/// Returned from [`ModelId::try_new`] when the input is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("model id must not be empty or whitespace-only")]
pub struct EmptyModelId;

impl ModelId {
    /// Construct a [`ModelId`], panicking on empty input. Prefer
    /// [`ModelId::try_new`] in production paths.
    ///
    /// # Panics
    /// Panics if `s` is empty or whitespace-only.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self::try_new(s.into()).expect("non-empty model id")
    }

    /// Construct a [`ModelId`], returning [`EmptyModelId`] on bad input.
    ///
    /// # Errors
    /// Returns [`EmptyModelId`] if `s` is empty after `trim`.
    pub fn try_new(s: impl Into<String>) -> Result<Self, EmptyModelId> {
        let s: String = s.into();
        if s.trim().is_empty() {
            Err(EmptyModelId)
        } else {
            Ok(Self(Arc::from(s)))
        }
    }

    /// Borrow as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Cache key for a contiguous slice of KV state. Deterministic from
/// inputs so cross-session lookups hit the same Lago object.
///
/// Derivation: BLAKE3 over a length-prefixed concatenation of
/// `(model_id, anima_did, prompt_bytes, range.start, range.end)`. The
/// 32-byte digest is the key. `AnimaId` is part of the key so KV is
/// scoped to identity per L5-D6.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KvKey([u8; 32]);

impl KvKey {
    /// Derive a key from the canonical inputs.
    ///
    /// `model_id` and `anima_did` are public-knowledge identifiers;
    /// `prompt_bytes` is whatever wire-form the backend uses for the
    /// prefix; `range` is the position interval within the cached
    /// sequence.
    ///
    /// # Panics
    /// Panics if any input length exceeds `u32::MAX` bytes or `usize`
    /// values exceed `u64` (impossible on supported targets).
    #[must_use]
    pub fn derive(
        model_id: &str,
        anima_did: &str,
        prompt_bytes: &[u8],
        range: Range<usize>,
    ) -> Self {
        // BLAKE3 keyed hash with a Spec-E namespace constant. Keyed
        // hashing prevents key forgery from controlled input.
        let mut hasher = blake3::Hasher::new_keyed(b"inference-core::KvKey::v1\0\0\0\0\0\0\0");
        hasher.update(&u32::try_from(model_id.len()).unwrap().to_le_bytes());
        hasher.update(model_id.as_bytes());
        hasher.update(&u32::try_from(anima_did.len()).unwrap().to_le_bytes());
        hasher.update(anima_did.as_bytes());
        hasher.update(&u32::try_from(prompt_bytes.len()).unwrap().to_le_bytes());
        hasher.update(prompt_bytes);
        hasher.update(&u64::try_from(range.start).unwrap().to_le_bytes());
        hasher.update(&u64::try_from(range.end).unwrap().to_le_bytes());
        let bytes: [u8; 32] = hasher.finalize().into();
        Self(bytes)
    }

    /// Hex-encoded 32-byte digest (64 chars).
    #[must_use]
    pub fn hex(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(64);
        for b in self.0 {
            write!(s, "{b:02x}").expect("writing to a String never fails");
        }
        s
    }
}

impl std::fmt::Display for KvKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}
