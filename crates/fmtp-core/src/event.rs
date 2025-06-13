use std::time::Instant;

use anyhow::anyhow;

use crate::{Config, FmtpMessage, FmtpPacket, FmtpType};

/// Commands that can be issued by the user to control the FMTP connection.
///
/// These commands correspond to the MT-* service primitives defined in the
/// EUROCONTROL FMTP specification.
#[derive(Debug)]
pub enum UserCommand {
    /// Request to establish an FMTP connection (MT-CON service primitive).
    Setup {
        /// The identifier of the connection configuration to use
        id: String,
    },

    /// Request to stop an existing FMTP Association and release the underlying
    /// connection (MT-DIS service primitive).
    Disconnect,

    /// Request to send a message over an existing FMTP Association (MT-DATA service primitive).
    Data {
        /// The message to send
        msg: FmtpMessage,
    },

    /// Request to stop an existing FMTP Association without releasing the
    /// underlying connection (MT-STOP service primitive).
    Shutdown,

    /// Request to establish an FMTP Association over an established
    /// FMTP connection (MT-ASSOC service primitive).
    Startup,
}

/// Events that drive the FMTP connection state machine.
///
/// These events include both protocol events (received messages, timer expirations)
/// and user commands. Each event may trigger state transitions and actions according
/// to the FMTP state machine specification.
#[derive(Debug)]
pub enum Event {
    /// A TCP transport connection establishment indication has been received
    RSetup {
        /// The time when the event occurred
        now: Instant,
    },

    /// A TCP transport connection release indication has been received
    RDisconnect,

    /// A REJECT identification message has been received
    RReject,

    /// An ACCEPT identification message has been received
    /// from the remote peer
    RAccept {
        /// The time when the event occurred
        now: Instant,
    },

    /// An identification message containing a valid identification
    /// value for the peer MT-User has been received
    RIdValid {
        /// The time when the event occurred
        now: Instant,
        /// The identifier of the connection
        id: String,
    },

    /// An identification message which fails the identification
    /// value validation test has been received.
    RIdInvalid,

    /// An [`FmtpMessage`] (Operational or Operator message
    /// type) has been received from the remote user
    RData {
        /// The time when the event occurred
        now: Instant,
        /// The received message
        msg: FmtpMessage,
    },
    /// A HEARTBEAT message has been received from the remote system
    RHeartbeat {
        /// The time when the event occurred
        now: Instant,
    },

    /// A SHUTDOWN message has been received from the remote system
    RShutdown {
        /// The time when the event occurred
        now: Instant,
    },

    /// A STARTUP message has been received from the remote system
    RStartup {
        /// The time when the event occurred
        now: Instant,
    },

    /// Timer Ti (identification timeout) has expired
    TiTimeout,

    /// Timer Tr (response timeout) has expired
    TrTimeout {
        /// The time when the event occurred
        now: Instant,
    },

    /// Timer Ts (send heartbeat timeout) has expired
    TsTimeout {
        /// The time when the event occurred
        now: Instant,
    },

    /// Data transfer requested by user (MT-DATA service primitive).
    LData {
        /// The time when the event occurred
        now: Instant,
        /// The message to send
        msg: FmtpMessage,
    },

    /// Shutdown requested by user (MT-STOP service primitive)
    LShutdown {
        /// The time when the event occurred
        now: Instant,
    },

    /// Startup requested by user (MT-ASSOC service primitive)
    LStartup {
        /// The time when the event occurred
        now: Instant,
    },

    /// Request to establish an FMTP connection (MT-CON service primitive).
    LSetup {
        /// The identifier of the connection configuration to use
        id: String,
    },

    /// Request to stop an existing FMTP Association and release the underlying
    /// connection (MT-DIS service primitive).
    LDisconnect,
}
impl Event {
    /// Translates an incoming [`FmtpPacket`] to an [`Event`], 0 bytes
    /// length will be handled as `RDisconnect`.
    ///
    /// # Errors
    /// Returns an error if the packet cannot be converted to an [`FmtpMessage`], or if an unexpected system packet payload is encountered.
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
                msg: packet.try_to_msg()?,
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
