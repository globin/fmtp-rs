use std::ops::Deref;

use anyhow::anyhow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtpIdentifier(Vec<u8>);

impl FmtpIdentifier {
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
