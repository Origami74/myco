//! Neighborhood ids, as the neighborhood protocol makes them.
//!
//! - **Random:** 32 random bytes, for a neighborhood someone starts. Two with
//!   the same name never merge by accident.
//! - **From a name:** `sha256("nostr-neighborhood:" || lowercase name)`, for
//!   auto-accepted neighborhoods: anyone using the same name lands in the
//!   same one. `nearby` is one of these.

use std::fmt;

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// What a name-derived id is hashed under.
const NAME_PREFIX: &[u8] = b"nostr-neighborhood:";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NeighborhoodId([u8; 32]);

impl NeighborhoodId {
    /// A fresh random id.
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The id every participant derives from `name`. Case and surrounding
    /// whitespace don't count: "Lowlands 2027" and " lowlands 2027" meet.
    pub fn from_name(name: &str) -> Self {
        let mut h = Sha256::new();
        h.update(NAME_PREFIX);
        h.update(name.trim().to_lowercase().as_bytes());
        Self(h.finalize().into())
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let bytes = hex::decode(s).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }
}

impl fmt::Debug for NeighborhoodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NeighborhoodId({})", &self.to_hex()[..12])
    }
}

impl Serialize for NeighborhoodId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for NeighborhoodId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom("neighborhood id: 64 hex chars"))
    }
}

/// A random lowercase-hex token, for per-napplet ids and addresses.
pub fn token() -> String {
    let mut bytes = [0u8; 10];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_meet_regardless_of_case_and_padding() {
        assert_eq!(
            NeighborhoodId::from_name("Lowlands 2027"),
            NeighborhoodId::from_name(" lowlands 2027 ")
        );
        assert_ne!(
            NeighborhoodId::from_name("home"),
            NeighborhoodId::from_name("homes")
        );
    }

    #[test]
    fn random_ids_differ_and_round_trip() {
        let a = NeighborhoodId::random();
        assert_ne!(a, NeighborhoodId::random());
        assert_eq!(NeighborhoodId::from_hex(&a.to_hex()), Some(a));
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<NeighborhoodId>(&json).unwrap(), a);
        assert!(NeighborhoodId::from_hex("abcd").is_none());
    }

    #[test]
    fn tokens_are_address_safe() {
        let t = token();
        assert_eq!(t.len(), 20);
        assert!(t
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }
}
