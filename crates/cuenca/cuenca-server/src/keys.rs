//! Per-principal bearer keys (spec section 0, "Rev 2.2, first build").
//!
//! The owner mints one key per principal through the admin socket. A key is bound at
//! minting to one Tailscale stable node ID and carries the principal's read grant. The
//! keys file holds a SHA-256 of each key's secret, never the secret: the token is shown
//! once, to the owner who minted it.
//!
//! One key per principal, and a principal name is never reused: revoking a key revokes
//! its principal in the log ([`cuenca_core::Store::revoke`]), which refuses every later
//! write the principal asks for, and the authority refuses every request whose
//! principal the log names as revoked, whatever the keys file says. The log is the
//! record of revocation; the file only lists the keys that may still authenticate.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use cuenca_core::{ForkName, LocationName, NodeId, PrincipalName};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What a principal is (spec section 0). The owner is not one of these: owner acts go
/// through the admin socket, which no key opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// Reads main of the locations it is granted, the forks its grant names and the
    /// forks it holds now (its own drafts fenced after a takeover land in a sibling it
    /// holds); creates forks of granted locations.
    Agent,
    /// One of the owner's devices: reads every location and fork, and writes like an
    /// agent (it holds only the forks it creates; it lands nothing on its own).
    OwnerDevice,
}

/// What an agent's key may read, beyond the forks it holds now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Locations whose main it reads and which it may fork.
    #[serde(default)]
    pub locations: BTreeSet<LocationName>,
    /// Other forks it may read, such as its swarm's.
    #[serde(default)]
    pub forks: BTreeSet<ForkName>,
}

/// One minted key, as the keys file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRecord {
    /// Public identifier, the first part of the token.
    pub id: String,
    pub principal: PrincipalName,
    pub kind: PrincipalKind,
    /// The only node the key is accepted from.
    pub node: NodeId,
    pub grant: Grant,
    pub minted_at_ms: u64,
    /// Hex SHA-256 of the secret part of the token.
    secret_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("principal {0:?} already has a key; a principal name is never reused")]
    PrincipalTaken(String),
    #[error("keys file: {0}")]
    Io(#[from] io::Error),
}

const TOKEN_PREFIX: &str = "cuenca";
const ID_BYTES: usize = 8;
const SECRET_BYTES: usize = 32;

/// The keys file and its contents in memory. Every change is written to a temporary
/// file, fsynced and renamed over the file before it takes effect.
pub struct KeyRing {
    path: PathBuf,
    keys: RwLock<BTreeMap<String, KeyRecord>>,
}

impl KeyRing {
    /// Open the keys file at `path`, or start empty if it does not exist. A file that
    /// anyone but its owner can read or write is refused.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let keys = match std::fs::read(&path) {
            Ok(bytes) => {
                let mode = std::fs::metadata(&path)?.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "keys file {} has mode {:o}; it must be 0600",
                            path.display(),
                            mode & 0o777
                        ),
                    ));
                }
                let list: Vec<KeyRecord> = serde_json::from_slice(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                list.into_iter().map(|k| (k.id.clone(), k)).collect()
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            keys: RwLock::new(keys),
        })
    }

    /// Mint a key for a new principal. Returns the record and the token, which exists
    /// nowhere else: the file keeps only its hash. Whether the log has revoked the
    /// principal is the caller's to check first.
    pub fn mint(
        &self,
        principal: PrincipalName,
        kind: PrincipalKind,
        node: NodeId,
        grant: Grant,
    ) -> Result<(KeyRecord, String), KeyError> {
        let mut keys = self.keys.write().expect("keys lock");
        if keys.values().any(|k| k.principal == principal) {
            return Err(KeyError::PrincipalTaken(principal.to_string()));
        }
        let mut id = [0u8; ID_BYTES];
        let mut secret = [0u8; SECRET_BYTES];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut id);
        rng.fill_bytes(&mut secret);
        let (id, secret) = (hex::encode(id), hex::encode(secret));
        let record = KeyRecord {
            id: id.clone(),
            principal,
            kind,
            node,
            grant,
            minted_at_ms: now_ms(),
            secret_sha256: sha256_hex(secret.as_bytes()),
        };
        let mut next = keys.clone();
        next.insert(id.clone(), record.clone());
        write_atomically(&self.path, &next)?;
        *keys = next;
        Ok((record, format!("{TOKEN_PREFIX}_{id}_{secret}")))
    }

    /// Remove `principal`'s key from the file. Returns whether it had one.
    pub fn remove(&self, principal: &PrincipalName) -> Result<bool, KeyError> {
        let mut keys = self.keys.write().expect("keys lock");
        let mut next = keys.clone();
        next.retain(|_, k| &k.principal != principal);
        if next.len() == keys.len() {
            return Ok(false);
        }
        write_atomically(&self.path, &next)?;
        *keys = next;
        Ok(true)
    }

    /// The key a token names, if the token is well formed and its secret matches.
    pub fn verify(&self, token: &str) -> Option<KeyRecord> {
        let mut parts = token.splitn(3, '_');
        let (prefix, id, secret) = (parts.next()?, parts.next()?, parts.next()?);
        if prefix != TOKEN_PREFIX {
            return None;
        }
        let keys = self.keys.read().expect("keys lock");
        let record = keys.get(id)?;
        let presented = sha256_hex(secret.as_bytes());
        constant_time_eq(presented.as_bytes(), record.secret_sha256.as_bytes())
            .then(|| record.clone())
    }

    pub fn list(&self) -> Vec<KeyRecord> {
        self.keys
            .read()
            .expect("keys lock")
            .values()
            .cloned()
            .collect()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write `keys` to `path`: a 0600 temporary file beside it, fsynced, renamed over
/// `path`, and the directory fsynced, so a crash leaves the old file or the new one.
fn write_atomically(path: &Path, keys: &BTreeMap<String, KeyRecord>) -> io::Result<()> {
    let list: Vec<&KeyRecord> = keys.values().collect();
    let json = serde_json::to_vec_pretty(&list).map_err(io::Error::other)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    // A temporary file left by an earlier crash keeps the mode it was created with.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(&json)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(s: &str) -> PrincipalName {
        PrincipalName::new(s).unwrap()
    }

    fn node() -> NodeId {
        NodeId::new("nNodeCNTRL").unwrap()
    }

    #[test]
    fn a_token_verifies_and_survives_a_reopen_but_its_secret_is_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.json");
        let ring = KeyRing::open(&path).unwrap();
        let (record, token) = ring
            .mint(
                principal("a1"),
                PrincipalKind::Agent,
                node(),
                Grant::default(),
            )
            .unwrap();
        assert_eq!(ring.verify(&token), Some(record.clone()));
        let secret = token.rsplit('_').next().unwrap();
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains(secret));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reopened = KeyRing::open(&path).unwrap();
        assert_eq!(reopened.verify(&token), Some(record));
    }

    #[test]
    fn wrong_or_malformed_tokens_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ring = KeyRing::open(dir.path().join("keys.json")).unwrap();
        let (_, token) = ring
            .mint(
                principal("a1"),
                PrincipalKind::Agent,
                node(),
                Grant::default(),
            )
            .unwrap();
        let mut flipped = token.clone().into_bytes();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
        let flipped = String::from_utf8(flipped).unwrap();
        let id = token.split('_').nth(1).unwrap();
        for bad in [
            "",
            "cuenca",
            &flipped,
            &token.replacen("cuenca", "other", 1),
            &format!("cuenca_{id}_"),
            &format!("cuenca_{id}"),
            &token[..token.len() - 1],
        ] {
            assert_eq!(ring.verify(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_principal_gets_one_key_and_removal_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.json");
        let ring = KeyRing::open(&path).unwrap();
        let (_, token) = ring
            .mint(
                principal("a1"),
                PrincipalKind::Agent,
                node(),
                Grant::default(),
            )
            .unwrap();
        assert!(matches!(
            ring.mint(
                principal("a1"),
                PrincipalKind::OwnerDevice,
                node(),
                Grant::default()
            ),
            Err(KeyError::PrincipalTaken(_))
        ));
        assert!(ring.remove(&principal("a1")).unwrap());
        assert!(!ring.remove(&principal("a1")).unwrap());
        assert_eq!(ring.verify(&token), None);
        assert_eq!(KeyRing::open(&path).unwrap().verify(&token), None);
    }

    #[test]
    fn a_keys_file_others_can_read_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.json");
        KeyRing::open(&path)
            .unwrap()
            .mint(
                principal("a1"),
                PrincipalKind::Agent,
                node(),
                Grant::default(),
            )
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(KeyRing::open(&path).is_err());
    }
}
