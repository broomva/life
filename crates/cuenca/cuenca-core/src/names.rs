use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// Longest location, fork or principal name, in bytes: room for a sibling fork's
/// name (`<fork>-from-<node>-e<epoch>`), well under any filesystem's 255.
pub const MAX_NAME_BYTES: usize = 128;

const NAME_RULE: &str =
    "1-128 bytes of a-z, 0-9, '.', '_' or '-', starting with a letter or digit, without '..'";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind} name {name:?} must be {rule}")]
pub struct NameError {
    kind: &'static str,
    name: String,
    rule: &'static str,
}

impl NameError {
    pub(crate) fn new(kind: &'static str, name: String, rule: &'static str) -> Self {
        Self { kind, name, rule }
    }
}

/// Whether `s` is safe as one directory name in the volume (`main/<location>`,
/// `forks/<fork>`) on the clients the spec names, macOS and iOS over Samba and Linux:
/// lowercase, so no two names fold together on APFS; ASCII, so normalization cannot
/// either; no separators, no dot-only names. Windows rules (no trailing dot, reserved
/// device names) are not applied.
fn valid(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME_BYTES
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
        && !s.contains("..")
}

macro_rules! name {
    ($(#[$doc:meta])* $t:ident, $kind:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $t(String);

        impl $t {
            pub fn new(s: impl Into<String>) -> Result<Self, NameError> {
                let s = s.into();
                if valid(&s) {
                    Ok(Self(s))
                } else {
                    Err(NameError::new($kind, s, NAME_RULE))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

name!(
    /// A registered location's name: any directory tree the authority tracks.
    LocationName,
    "location"
);
name!(
    /// A fork's name.
    ForkName,
    "fork"
);
name!(
    /// A principal: who holds keys and forks (an agent session, the owner). Minting and
    /// revoking its keys is the authority's (BRO-3251); the log records revocation.
    PrincipalName,
    "principal"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in [
            "broomva",
            "life",
            "a",
            "my-notes.v2",
            "x_1",
            &"a".repeat(128),
        ] {
            assert!(LocationName::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "-x",
            ".x",
            "a..b",
            "A",
            "a/b",
            "a\\b",
            "é",
            "a b",
            "a\0",
            &"a".repeat(129),
        ] {
            assert!(ForkName::new(bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<ForkName>("\"../x\"").is_err());
    }
}
