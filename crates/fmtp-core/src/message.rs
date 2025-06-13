use anyhow::anyhow;
use std::fmt::Display;

/// An FMTP message that can be sent over an established connection.
///
/// FMTP supports two types of messages:
/// - Operational: Used for normal operational data transfer
/// - Operator: Used for operator-to-operator communication
///
/// Both message types have a maximum payload size of 10240 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FmtpMessage {
    /// An operational message containing flight data
    Operational(Vec<u8>),
    /// An operator message for communication between operators
    Operator(Vec<u8>),
}

impl FmtpMessage {
    /// Creates a new operational message.
    ///
    /// # Arguments
    /// * `data` - The message payload. Must be 10240 bytes or less.
    ///
    /// # Errors
    /// Returns an error if the payload exceeds the maximum size.
    pub fn operational(data: Vec<u8>) -> anyhow::Result<Self> {
        // FIXME encoding?
        if data.len() > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }

        Ok(Self::Operational(data))
    }

    /// Creates a new operator message.
    ///
    /// # Arguments
    /// * `data` - The message payload. Must be 10240 bytes or less.
    ///
    /// # Errors
    /// Returns an error if the payload exceeds the maximum size.
    pub fn operator(data: Vec<u8>) -> anyhow::Result<Self> {
        // FIXME encoding?
        if data.len() > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }

        Ok(Self::Operator(data))
    }

    fn data(&self) -> &[u8] {
        match self {
            FmtpMessage::Operator(data) | FmtpMessage::Operational(data) => data,
        }
    }

    fn typ(&self) -> &str {
        match self {
            FmtpMessage::Operational(_) => "Operational",
            FmtpMessage::Operator(_) => "Operator",
        }
    }
}

impl Display for FmtpMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{}: {}",
            self.typ(),
            String::from_utf8_lossy(self.data())
        )
    }
}
