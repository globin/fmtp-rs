use std::{collections::HashMap, net::SocketAddr, time::Duration};

use crate::FmtpIdentifier;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Client,
    // bind_address as payload here?
    Server,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Idle,
    Ready,
    DataReady,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub local_id: FmtpIdentifier,
    pub remote_id: FmtpIdentifier,
    pub ti: Duration,
    pub tr: Duration,
    pub ts: Duration,
    pub remote_addresses: Vec<SocketAddr>,
    pub role: Role,
    pub initial_target: Target,
    pub connect_retry_timer: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub bind_address: Option<SocketAddr>,
    pub connections: HashMap<String, ConnectionConfig>,
    /// Initial connection timeout before the remote partner has identified itself and the
    /// connection config can be matched to the connection.
    pub server_ti: Option<Duration>,
}

// TODO
// new: validate (or auto-fill) server_ti and bind_address for servers
// bind_address -> ConnectionConfig?
