use std::fmt::Display;

use anyhow::anyhow;
use tracing::{debug, trace};
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes, big_endian::U16};

use crate::{Config, ConnectionConfig, FmtpIdentifier, FmtpMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(u8)]
enum FmtpVersion {
    X02 = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(u8)]
enum FmtpReserved {
    X00 = 0x00,
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
pub enum FmtpType {
    Operational = 1,
    Operator,
    Identification,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(C, packed)]
pub struct FmtpPacketHeader {
    version: FmtpVersion,
    _reserved: FmtpReserved,
    length: U16,
    pub typ: FmtpType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtpPacket {
    pub header: FmtpPacketHeader,
    data: Vec<u8>,
}
impl FmtpPacket {
    const SHUTDOWN: &[u8] = b"00";
    const STARTUP: &[u8] = b"01";
    const HEARTBEAT: &[u8] = b"03";
    const ACCEPT: &[u8] = b"ACCEPT";
    const REJECT: &[u8] = b"REJECT";
    pub fn new(typ: FmtpType, data: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let data = data.into();

        // FIXME encoding?
        if data.len() > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }
        Ok(Self {
            header: FmtpPacketHeader {
                version: FmtpVersion::X02,
                _reserved: FmtpReserved::X00,
                #[expect(clippy::cast_possible_truncation, reason = "length checked above")]
                length: (5 + data.len() as u16).into(),
                typ,
            },
            data,
        })
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        let mut packet = self.header.as_bytes().to_owned();
        packet.extend(self.data);
        packet
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let (header, rest) = FmtpPacketHeader::try_ref_from_prefix(bytes)
            .map_err(|e| anyhow!("Could not decode packet: {e}"))?;

        if header.length > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }
        //
        // FIXME copy-less ref
        Ok(FmtpPacket {
            header: header.clone(),
            data: rest[..(header.length.get() as usize - 5)].to_owned(),
        })
    }

    pub fn from_msg(msg: FmtpMessage) -> Self {
        // encoding and length is safe
        let (typ, bytes) = match msg {
            FmtpMessage::Operational(bytes) => (FmtpType::Operational, bytes),
            FmtpMessage::Operator(bytes) => (FmtpType::Operator, bytes),
        };
        let packet = FmtpPacket::new(typ, bytes).unwrap();
        debug!("Data packet: {packet}");

        packet
    }
    pub fn to_msg(self) -> anyhow::Result<FmtpMessage> {
        let msg = match self.header.typ {
            FmtpType::Operational => FmtpMessage::Operational(self.data),
            FmtpType::Operator => FmtpMessage::Operator(self.data),
            FmtpType::Identification => Err(anyhow!(
                "Identification packet cannot be converted to FmtpMessage"
            ))?,
            FmtpType::System => Err(anyhow!("System packet cannot be converted to FmtpMessage"))?,
        };

        Ok(msg)
    }

    // TYP == Identification
    pub fn handshake(local_id: &FmtpIdentifier, remote_id: &FmtpIdentifier) -> Self {
        let mut payload = (**local_id).to_owned();
        payload.push(b'-');
        payload.extend(&**remote_id);
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::Identification, payload).unwrap();
        debug!("Handshake packet: {packet}");

        packet
    }
    pub fn accept() -> Self {
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::Identification, Self::ACCEPT).unwrap();
        debug!("Handshake accept packet: {packet}");

        packet
    }
    pub fn reject() -> Self {
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::Identification, Self::REJECT).unwrap();
        debug!("Handshake reject packet: {packet}");

        packet
    }
    #[must_use]
    pub fn check_handshake<'config>(
        &self,
        config: &'config Config,
        connection_id: Option<&'config str>,
    ) -> Option<&'config str> {
        let check = |conn_config: &'config ConnectionConfig,
                     connection: Option<&'config str>|
         -> Option<&'config str> {
            let mut data: Vec<u8> = (*conn_config.remote_id).to_owned();
            data.push(b'-');
            data.extend(&*conn_config.local_id);
            trace!(
                "checking handshake: {} == {}",
                self,
                String::from_utf8_lossy(&data)
            );
            if self.header.typ == FmtpType::Identification && self.data == data {
                connection
            } else {
                None
            }
        };

        if let Some(conn_config) = connection_id.and_then(|c| config.connections.get(c)) {
            check(conn_config, connection_id)
        } else {
            config
                .connections
                .iter()
                .find_map(|(connection, conn_config)| check(conn_config, Some(connection)))
        }
    }
    #[must_use]
    pub fn is_accept(&self) -> bool {
        self.header.typ == FmtpType::Identification && self.data == Self::ACCEPT
    }
    #[must_use]
    pub fn is_reject(&self) -> bool {
        self.header.typ == FmtpType::Identification && self.data == Self::REJECT
    }

    // TYP == System
    pub fn startup() -> Self {
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::System, Self::STARTUP).unwrap();
        debug!("Startup packet: {packet}");

        packet
    }
    pub fn shutdown() -> Self {
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::System, Self::SHUTDOWN).unwrap();
        debug!("Shutdown packet: {packet}");

        packet
    }
    pub fn heartbeat() -> Self {
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::System, Self::HEARTBEAT).unwrap();
        debug!("Heartbeat packet: {packet}");

        packet
    }
    #[must_use]
    pub fn is_startup(&self) -> bool {
        self.header.typ == FmtpType::System && self.data == Self::STARTUP
    }
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.header.typ == FmtpType::System && self.data == Self::SHUTDOWN
    }
    #[must_use]
    pub fn is_heartbeat(&self) -> bool {
        self.header.typ == FmtpType::System && self.data == Self::HEARTBEAT
    }
}

impl Display for FmtpPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{:?}: {}",
            self.header.typ,
            String::from_utf8(self.data.clone()).unwrap_or(format!("{:?}", self.data))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmtp_type_to_u8() {
        assert_eq!(FmtpType::Operational as u8, 1);
        assert_eq!(FmtpType::Operator as u8, 2);
        assert_eq!(FmtpType::Identification as u8, 3);
        assert_eq!(FmtpType::System as u8, 4);
    }

    #[test]
    fn fmtp_header_to_bytes() {
        let header = FmtpPacketHeader {
            version: FmtpVersion::X02,
            _reserved: FmtpReserved::X00,
            length: 5.into(),
            typ: FmtpType::Operator,
        };
        assert_eq!([2, 0, 0, 5, 2], header.as_bytes());
    }
}
