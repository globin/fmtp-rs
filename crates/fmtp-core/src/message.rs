use std::fmt::Display;

use anyhow::anyhow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FmtpMessage {
    Operational(Vec<u8>),
    Operator(Vec<u8>),
}
impl FmtpMessage {
    pub fn operational(data: Vec<u8>) -> anyhow::Result<Self> {
        // FIXME encoding?
        if data.len() > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }

        Ok(Self::Operational(data))
    }

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
