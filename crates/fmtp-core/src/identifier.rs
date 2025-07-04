use std::{fmt, ops::Deref};

use anyhow::anyhow;
use serde::{Deserialize, de::Visitor};

/// An FMTP identifier used for connection handshaking.
///
/// This type represents a valid FMTP identifier that must be:
/// - Between 1 and 32 bytes in length
/// - Contain only ASCII characters
///
/// FMTP identifiers are used during the handshake phase to identify the endpoints
/// of a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtpIdentifier(Vec<u8>);

impl FmtpIdentifier {
    /// Creates a new FMTP identifier from the given bytes.
    ///
    /// # Arguments
    /// * `id` - The identifier bytes. Must be 1-32 bytes long and contain only ASCII characters.
    ///
    /// # Errors
    /// Returns [`Err`] if the input is empty, too long, or contains non-ASCII characters
    pub fn new(id: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let id = id.into();
        if id.is_empty() || id.len() > 32 {
            return Err(anyhow!("FMTP identifier must be 0<n<=32 bytes long"));
        }
        if !id.is_ascii() {
            return Err(anyhow!(
                "FMTP identifier must only contain bytes in the ASCII range"
            ));
        }
        Ok(Self(id))
    }
}

impl Deref for FmtpIdentifier {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for FmtpIdentifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = deserializer.deserialize_bytes(BytesVisitor)?;
        FmtpIdentifier::new(bytes).map_err(serde::de::Error::custom)
    }
}

struct BytesVisitor;
impl Visitor<'_> for BytesVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(
            "FMTP identifier must be 0<n<=32 bytes long and only contain bytes in the ASCII range",
        )
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.as_bytes().to_vec())
    }
}
