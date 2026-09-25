//! Deterministic 128-bit truncated BLAKE3 symbol identifier.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{LoomError, Result};

/// A 128-bit truncated BLAKE3 hash uniquely and deterministically identifying a symbol.
///
/// $$\text{SymbolID} = \text{BLAKE3}(\text{file\_path} \parallel \text{namespace\_hierarchy} \parallel \text{symbol\_name} \parallel \text{signature})$$
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub [u8; 16]);

impl SymbolId {
    /// Constructs a `SymbolId` from raw 16 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the raw 16 bytes of the `SymbolId`.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Returns the inner 16-byte array.
    #[must_use]
    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Returns a 32-character lowercase hexadecimal string representation.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parses a 32-character hexadecimal string into a `SymbolId`.
    ///
    /// # Errors
    /// Returns [`LoomError::InvalidSymbolIdHex`] if the hex string is malformed or invalid length.
    pub fn from_hex(hex_str: &str) -> Result<Self> {
        let mut bytes = [0u8; 16];
        hex::decode_to_slice(hex_str, &mut bytes)
            .map_err(|_| LoomError::InvalidSymbolIdHex(hex_str.to_string()))?;
        Ok(Self(bytes))
    }

    /// Deterministically computes a `SymbolId` from its canonical path, namespace hierarchy, name, and signature.
    ///
    /// Boundaries between components are delimited to guarantee collision resistance across variable-length fields.
    #[must_use]
    pub fn derive(
        file_path: &str,
        namespace_hierarchy: &[&str],
        symbol_name: &str,
        signature: &str,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();

        // Delimit file path
        hasher.update(file_path.as_bytes());
        hasher.update(b"\0");

        // Delimit namespace hierarchy
        for ns in namespace_hierarchy {
            hasher.update(ns.as_bytes());
            hasher.update(b"::");
        }
        hasher.update(b"\0");

        // Delimit symbol name
        hasher.update(symbol_name.as_bytes());
        hasher.update(b"\0");

        // Delimit signature
        hasher.update(signature.as_bytes());

        let hash_output = hasher.finalize();
        let mut truncated = [0u8; 16];
        truncated.copy_from_slice(&hash_output.as_bytes()[..16]);
        Self(truncated)
    }
}

impl fmt::Debug for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SymbolId({})", self.to_hex())
    }
}

impl fmt::Display for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl FromStr for SymbolId {
    type Err = LoomError;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_hex(s)
    }
}

impl TryFrom<&[u8]> for SymbolId {
    type Error = LoomError;

    fn try_from(slice: &[u8]) -> Result<Self> {
        if slice.len() != 16 {
            return Err(LoomError::InvalidSymbolIdLength(slice.len()));
        }
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(slice);
        Ok(Self(bytes))
    }
}

impl Serialize for SymbolId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_hex())
        } else {
            serializer.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for SymbolId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            Self::from_hex(&s).map_err(serde::de::Error::custom)
        } else {
            struct BytesVisitor;

            impl<'de> serde::de::Visitor<'de> for BytesVisitor {
                type Value = SymbolId;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("a 16-byte slice for SymbolId")
                }

                fn visit_bytes<E>(self, v: &[u8]) -> std::result::Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    SymbolId::try_from(v).map_err(serde::de::Error::custom)
                }

                fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
                where
                    A: serde::de::SeqAccess<'de>,
                {
                    let mut bytes = [0u8; 16];
                    for (i, byte) in bytes.iter_mut().enumerate() {
                        *byte = seq
                            .next_element()?
                            .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
                    }
                    Ok(SymbolId(bytes))
                }
            }

            deserializer.deserialize_bytes(BytesVisitor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_hashing_consistency() {
        let id1 = SymbolId::derive(
            "src/service/auth.rs",
            &["service", "auth"],
            "authenticate_user",
            "pub fn authenticate_user(req: AuthRequest) -> Result<User>",
        );

        let id2 = SymbolId::derive(
            "src/service/auth.rs",
            &["service", "auth"],
            "authenticate_user",
            "pub fn authenticate_user(req: AuthRequest) -> Result<User>",
        );

        assert_eq!(id1, id2);
        assert_eq!(id1.to_hex(), id2.to_hex());
    }

    #[test]
    fn test_hash_distinguishes_different_signatures() {
        let id1 = SymbolId::derive("src/math.rs", &[], "compute", "fn compute(x: i32) -> i32");
        let id2 = SymbolId::derive("src/math.rs", &[], "compute", "fn compute(x: f64) -> f64");

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_hash_distinguishes_namespaces() {
        let id1 = SymbolId::derive("src/lib.rs", &["foo", "bar"], "run", "fn run()");
        let id2 = SymbolId::derive("src/lib.rs", &["foobar"], "run", "fn run()");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_hex_conversion_roundtrip() {
        let original = SymbolId::derive("src/main.rs", &[], "main", "fn main()");
        let hex_repr = original.to_hex();
        assert_eq!(hex_repr.len(), 32);

        let parsed: SymbolId = hex_repr.parse().expect("valid hex parse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_invalid_hex() {
        assert!(SymbolId::from_hex("invalid_hex").is_err());
        assert!(SymbolId::from_hex("1234").is_err()); // Too short
    }

    #[test]
    fn test_json_serde_serialization() {
        let id = SymbolId::derive("src/main.rs", &[], "main", "fn main()");
        let json = serde_json::to_string(&id).expect("serialize to json");
        assert_eq!(json, format!("\"{}\"", id.to_hex()));

        let deserialized: SymbolId = serde_json::from_str(&json).expect("deserialize from json");
        assert_eq!(id, deserialized);
    }
}
