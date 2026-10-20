use crate::error::TreeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha1_checked::{CollisionResult, Sha1};
use sha2::Digest as _;
use sha2::Sha256;
use std::fmt;

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex<const N: usize>(s: &str) -> Result<[u8; N], TreeError> {
    let bad = || TreeError::BadOid(s.to_string());
    if s.len() != N * 2
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad());
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

macro_rules! hex_id {
    ($name:ident, $len:expr) => {
        impl $name {
            /// Parse a lowercase hex string.
            pub fn from_hex(s: &str) -> Result<Self, TreeError> {
                decode_hex::<$len>(s).map(Self)
            }

            /// The id as lowercase hex.
            pub fn to_hex(&self) -> String {
                encode_hex(&self.0)
            }

            pub fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            pub fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::from_hex(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// A git object id: SHA-1 over `"<type> <len>\0"` followed by the object body.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitOid([u8; 20]);
hex_id!(GitOid, 20);

impl GitOid {
    /// The id of git's empty tree, `4b825dc642cb6eb9a060e54bf8d69288fbee4904`.
    pub fn empty_tree() -> Self {
        Self::for_object("tree", b"")
    }

    /// The id git gives an object of `kind` ("blob", "tree", ...) with this body. This
    /// is plain SHA-1, as git names objects; to accept untrusted content, use
    /// [`crate::BlobHasher`], which also runs SHA-1 collision detection as git does.
    pub fn for_object(kind: &str, body: &[u8]) -> Self {
        let mut h = Sha1::builder().detect_collision(false).build();
        sha1_checked::Digest::update(&mut h, format!("{kind} {}\0", body.len()).as_bytes());
        sha1_checked::Digest::update(&mut h, body);
        Self((*h.try_finalize().hash()).into())
    }

    /// The id git gives a blob with this content.
    pub fn for_blob(content: &[u8]) -> Self {
        Self::for_object("blob", content)
    }
}

/// The SHA-256 of a blob's content, as Lago's blob store keys it (lowercase hex).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);
hex_id!(ContentHash, 32);

impl ContentHash {
    pub fn of(content: &[u8]) -> Self {
        Self(Sha256::digest(content).into())
    }
}

/// A SHA-1 hasher with git's collision detection (SHA-1DC). With `safe_hash` off, the
/// digest of colliding input is still plain SHA-1, but the result is flagged.
pub(crate) fn checked_sha1() -> Sha1 {
    Sha1::builder().safe_hash(false).build()
}

/// The digest, or `None` if collision detection fired.
pub(crate) fn finish_checked(h: Sha1) -> Option<[u8; 20]> {
    match h.try_finalize() {
        CollisionResult::Ok(digest) => Some(digest.into()),
        CollisionResult::Mitigated(_) | CollisionResult::Collision(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector on its own. No public collision survives git's `blob <len>` header,
    /// so this and the `BlobHasher` test in entry.rs are the end-to-end evidence.
    #[test]
    fn collision_detection_fires_on_a_known_collision() {
        // The SHA-1 chosen-prefix collision from "SHA-1 is a Shambles" (2020).
        let mut h = checked_sha1();
        sha1_checked::Digest::update(&mut h, include_bytes!("../tests/data/sha-mbles-1.bin"));
        assert_eq!(finish_checked(h), None);
        let mut ok = checked_sha1();
        sha1_checked::Digest::update(&mut ok, b"hello\n");
        assert!(finish_checked(ok).is_some());
    }

    #[test]
    fn known_git_ids() {
        assert_eq!(
            GitOid::empty_tree().to_hex(),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
        // `echo hello | git hash-object --stdin`
        assert_eq!(
            GitOid::for_blob(b"hello\n").to_hex(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            GitOid::for_blob(b"").to_hex(),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }

    #[test]
    fn content_hash_matches_sha256() {
        assert_eq!(
            ContentHash::of(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hex_round_trip_and_rejects() {
        let id = GitOid::for_blob(b"x");
        assert_eq!(GitOid::from_hex(&id.to_hex()).unwrap(), id);
        assert!(GitOid::from_hex("abc").is_err());
        assert!(
            GitOid::from_hex(&"A".repeat(40)).is_err(),
            "uppercase is not canonical"
        );
        assert!(GitOid::from_hex(&"g".repeat(40)).is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<GitOid>(&json).unwrap(), id);
    }
}
