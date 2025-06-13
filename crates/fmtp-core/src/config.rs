use std::{collections::HashMap, net::SocketAddr, time::Duration};

use crate::FmtpIdentifier;

/// The role of the FMTP connection endpoint.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// Client role - initiates connections to servers
    Client,
    /// Server role - accepts incoming connections
    Server,
}

/// The target state for the FMTP connection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// No connection established
    Idle,
    /// Connection established but not transferring data
    Ready,
    /// Connection established and ready for data transfer
    DataReady,
}

/// Configuration for a single FMTP connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionConfig {
    /// Local FMTP identifier used in handshake
    pub local_id: FmtpIdentifier,
    /// Remote FMTP identifier used in handshake
    pub remote_id: FmtpIdentifier,
    /// Timer for identification phase (waiting for identification)
    pub ti: Duration,
    /// Timer for response (waiting for heartbeat or data)
    pub tr: Duration,
    /// Timer for sending heartbeats, if no other data is sent
    pub ts: Duration,
    /// List of remote addresses to try to connect to
    pub remote_addresses: Vec<SocketAddr>,
    /// Role of this endpoint (Client or Server)
    pub role: Role,
    /// Initial target state for the connection
    pub initial_target: Target,
    /// Optional timer for connection retry attempts
    pub connect_retry_timer: Option<Duration>,
}

/// Global FMTP configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Server bind address for all server connections
    pub bind_address: Option<SocketAddr>,
    /// Map of connection configurations by connection ID
    pub connections: HashMap<String, ConnectionConfig>,
    /// Initial connection timeout before the remote partner has identified itself and the
    /// connection config can be matched to the connection. Only used by servers.
    pub server_ti: Option<Duration>,
}

// TODO
// new: validate (or auto-fill) server_ti and bind_address for servers
// bind_address -> ConnectionConfig?
