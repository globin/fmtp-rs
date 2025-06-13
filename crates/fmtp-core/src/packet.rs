use std::fmt::Display;

use anyhow::anyhow;
use tracing::trace;
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes, big_endian::U16};

use crate::{Config, ConnectionConfig, FmtpIdentifier, FmtpMessage};

/// FMTP protocol version. (always 2 in this implementation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(u8)]
enum FmtpVersion {
    X02 = 0x02,
}

/// Reserved field in FMTP packet header. (always 0 in this implementation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(u8)]
enum FmtpReserved {
    X00 = 0x00,
}

/// FMTP packet type.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
pub enum FmtpType {
    /// Operational message packet (e.g. OLDI messages)
    Operational = 1,
    /// Operator message packet (operator communication)
    Operator,
    /// Identification packet (connection handshaking)
    Identification,
    /// System packet (protocol control)
    System,
}

/// FMTP packet header structure.
#[derive(Clone, Debug, PartialEq, Eq, IntoBytes, KnownLayout, Immutable, TryFromBytes)]
#[repr(C, packed)]
pub struct FmtpPacketHeader {
    /// (1 byte) Protocol version
    version: FmtpVersion,
    /// (1 byte) Reserved field, must be 0
    _reserved: FmtpReserved,
    /// (2 bytes) Length of the packet data in big-endian
    length: U16,
    /// (1 byte) Packet type
    pub typ: FmtpType,
}

/// A complete FMTP packet.
///
/// An FMTP packet consists of a 5-byte header followed by variable-length data.
/// The data length is specified in the header and must not exceed 10240 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtpPacket {
    /// 5 byte header containing protocol version, reserved field, length, and type
    pub header: FmtpPacketHeader,
    data: Vec<u8>,
}

impl FmtpPacket {
    const SHUTDOWN: &[u8] = b"00";
    const STARTUP: &[u8] = b"01";
    const HEARTBEAT: &[u8] = b"03";
    const ACCEPT: &[u8] = b"ACCEPT";
    const REJECT: &[u8] = b"REJECT";
    /// Creates a new FMTP packet.
    ///
    /// # Arguments
    /// * `typ` - The packet [`FmtpType`]
    /// * `data` - The packet data
    ///
    /// # Errors
    /// Returns an [`Err`] if the data is empty or exceeds the maximum size of 10240 bytes.
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

    /// Converts the packet into a byte vector.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        let mut packet = self.header.as_bytes().to_owned();
        packet.extend(self.data);
        packet
    }

    /// Creates a packet from raw bytes.
    ///
    /// # Arguments
    /// * `bytes` - The raw packet bytes
    ///
    /// # Errors
    /// Returns an [`Err`] if the header is malformed or the data length exceeds the maximum size
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let (header, rest) = FmtpPacketHeader::try_ref_from_prefix(bytes)
            .map_err(|e| anyhow!("Could not decode packet: {e}"))?;

        if header.length > 10240 {
            return Err(anyhow!("Payload must be of size 10240 bytes or less."));
        }

        // TODO check payload encoding
        // FIXME copy-less ref
        Ok(FmtpPacket {
            header: header.clone(),
            data: rest[..(header.length.get() as usize - 5)].to_owned(),
        })
    }

    /// Creates a packet from an FMTP message.
    #[expect(clippy::missing_panics_doc, reason = "length checked in FmtpMessage")]
    pub fn from_msg(msg: FmtpMessage) -> Self {
        // encoding and length is safe
        let (typ, bytes) = match msg {
            FmtpMessage::Operational(bytes) => (FmtpType::Operational, bytes),
            FmtpMessage::Operator(bytes) => (FmtpType::Operator, bytes),
        };
        let packet = FmtpPacket::new(typ, bytes).unwrap();
        trace!("Data packet: {packet}");

        packet
    }

    /// Tries to convert the packet into an FMTP message.
    ///
    /// # Errors
    /// Returns an [`Err`] if the packet type is `Identification` or `System`, as these cannot be converted to an `FmtpMessage`.
    pub fn try_to_msg(self) -> anyhow::Result<FmtpMessage> {
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
    /// Creates a handshake packet for connection establishment.
    ///
    /// # Arguments
    /// * `local_id` - The local identifier
    /// * `remote_id` - The remote identifier
    #[expect(
        clippy::missing_panics_doc,
        reason = "length checked in FmtpIdentifier"
    )]
    pub fn handshake(local_id: &FmtpIdentifier, remote_id: &FmtpIdentifier) -> Self {
        let mut payload = (**local_id).to_owned();
        payload.push(b'-');
        payload.extend(&**remote_id);
        // encoding and length is safe
        let packet = FmtpPacket::new(FmtpType::Identification, payload).unwrap();
        trace!("Handshake packet: {packet}");

        packet
    }
    /// Creates a handshake acceptance packet.
    #[expect(clippy::missing_panics_doc, reason = "length is static")]
    pub fn accept() -> Self {
        let packet = FmtpPacket::new(FmtpType::Identification, Self::ACCEPT).unwrap();
        trace!("Handshake accept packet: {packet}");

        packet
    }
    /// Creates a handshake rejection packet.
    #[expect(clippy::missing_panics_doc, reason = "length is static")]
    pub fn reject() -> Self {
        let packet = FmtpPacket::new(FmtpType::Identification, Self::REJECT).unwrap();
        trace!("Handshake reject packet: {packet}");

        packet
    }
    #[must_use]
    /// Checks if the packet is a valid handshake packet and returns the associated connection ID if so.
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
    /// Checks if the packet is a handshake acceptance packet.
    pub fn is_accept(&self) -> bool {
        self.header.typ == FmtpType::Identification && self.data == Self::ACCEPT
    }
    #[must_use]
    /// Checks if the packet is a handshake rejection packet.
    pub fn is_reject(&self) -> bool {
        self.header.typ == FmtpType::Identification && self.data == Self::REJECT
    }

    // TYP == System
    /// Creates a startup packet.
    #[expect(clippy::missing_panics_doc, reason = "length is static")]
    pub fn startup() -> Self {
        let packet = FmtpPacket::new(FmtpType::System, Self::STARTUP).unwrap();
        trace!("Startup packet: {packet}");

        packet
    }
    /// Creates a shutdown packet.
    #[expect(clippy::missing_panics_doc, reason = "length is static")]
    pub fn shutdown() -> Self {
        let packet = FmtpPacket::new(FmtpType::System, Self::SHUTDOWN).unwrap();
        trace!("Shutdown packet: {packet}");

        packet
    }
    /// Creates a heartbeat packet.
    #[expect(clippy::missing_panics_doc, reason = "length is static")]
    pub fn heartbeat() -> Self {
        let packet = FmtpPacket::new(FmtpType::System, Self::HEARTBEAT).unwrap();
        trace!("Heartbeat packet: {packet}");

        packet
    }
    #[must_use]
    /// Checks if the packet is a system startup packet.
    pub fn is_startup(&self) -> bool {
        self.header.typ == FmtpType::System && self.data == Self::STARTUP
    }
    #[must_use]
    /// Checks if the packet is a system shutdown packet.
    pub fn is_shutdown(&self) -> bool {
        self.header.typ == FmtpType::System && self.data == Self::SHUTDOWN
    }
    #[must_use]
    /// Checks if the packet is a system heartbeat packet.
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
