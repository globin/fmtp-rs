use std::time::Instant;

use anyhow::anyhow;

use crate::{Config, FmtpMessage, FmtpPacket, FmtpType};

#[derive(Debug)]
pub enum UserCommand {
    /// The MT-User requests the establishment of an FMTP connection. (MT-CON)
    Setup { id: String },
    /// The MT-User requests to stop an existing FMTP Association and
    /// release the underlying connection. (MT-DIS)
    Disconnect,
    /// The MT-User requests that an [`FmtpMessage`] (Operational or
    /// Operator message type) be sent from the local to the remote
    /// user over an existing FMTP Association. (MT-DATA)
    Data { now: Instant, msg: FmtpMessage },
    /// The MT-User requests to stop an existing FMTP
    /// Association without releasing the underlying connection. (MT-STOP)
    Shutdown { now: Instant },
    /// The MT-User requests the establishment of an FMTP
    /// Association over an established FMTP connection. (MT-ASSOC)
    Startup { now: Instant },
}

/// FMTP protocol events to advance the [`ConnectionState`] machine.
#[derive(Debug)]
pub enum Event {
    /// A TCP transport connection establishment indication has
    /// been received by the TCP client or server
    RSetup { now: Instant },
    /// A TCP transport connection release indication has been
    /// received
    RDisconnect,
    /// A REJECT identification message has been received.
    RReject,
    /// An ACCEPT identification message has been received
    /// from the remote peer.
    RAccept { now: Instant },
    /// An identification message containing a valid identification
    /// value for the peer MT-User has been received.
    RIdValid { now: Instant, id: String },
    /// An identification message which fails the identification
    /// value validation test has been received.
    RIdInvalid,
    /// An [`FmtpMessage`] (Operational or Operator message
    /// type) has been received from the remote user
    RData { now: Instant, msg: FmtpMessage },
    /// A HEARTBEAT message has been received from the remote system.
    RHeartbeat { now: Instant },
    /// A SHUTDOWN message has been received from the remote system.
    RShutdown { now: Instant },
    /// A STARTUP message has been received from the remote system.
    RStartup { now: Instant },
    /// Expiry of timer Ti when an Identification message is expected.
    TiTimeout,
    /// Expiry of timer Tr when a HEARTBEAT or a data message is expected.
    TrTimeout { now: Instant },
    /// Expiry of timer Ts for sending a HEARTBEAT to the remote user.
    TsTimeout { now: Instant },
    /// Manually triggered Events, or due to configured target or reconnection timeout
    UserCommand(UserCommand),
}
impl Event {
    /// Translates an incoming [`FmtpPacket`] to an [`Event`], 0 bytes
    /// length will be handled as `RDisconnect`.
    pub fn from_incoming_packet(
        bytes: usize,
        packet: FmtpPacket,
        now: Instant,
        config: &Config,
        connection_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        // connection closed
        if bytes == 0 {
            return Ok(Self::RDisconnect);
        }

        Ok(match packet.header.typ {
            FmtpType::Operational | FmtpType::Operator => Self::RData {
                now,
                msg: packet.to_msg()?,
            },
            FmtpType::Identification if packet.is_accept() => Self::RAccept { now },
            FmtpType::Identification if packet.is_reject() => Self::RReject,
            FmtpType::Identification => {
                if let Some(conn_id) = packet.check_handshake(config, connection_id) {
                    Self::RIdValid {
                        now,
                        id: conn_id.to_string(),
                    }
                } else {
                    Self::RIdInvalid
                }
                // FIXME address check
            }
            FmtpType::System if packet.is_startup() => Self::RStartup { now },
            FmtpType::System if packet.is_shutdown() => Self::RShutdown { now },
            FmtpType::System if packet.is_heartbeat() => Self::RHeartbeat { now },
            FmtpType::System => Err(anyhow!("unexpected system packet payload"))?,
        })
    }
}
