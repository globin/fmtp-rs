use std::{collections::HashMap, fs::read_to_string, net::SocketAddr, path::Path, time::Duration};

use anyhow::bail;
use serde::Deserialize;
use serde_with::{DurationSeconds, serde_as};

use crate::FmtpIdentifier;

/// The role of the FMTP connection endpoint.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Role {
    /// Client role - initiates connections to servers
    Client,
    /// Server role - accepts incoming connections
    Server,
}

/// The target state for the FMTP connection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum Target {
    /// No connection established
    Idle,
    /// Connection established but not transferring data
    Ready,
    /// Connection established and ready for data transfer
    DataReady,
}

/// Configuration for a single FMTP connection.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ConnectionConfig {
    /// Local FMTP identifier used in handshake
    pub local_id: FmtpIdentifier,
    /// Remote FMTP identifier used in handshake
    pub remote_id: FmtpIdentifier,
    /// Timer for identification phase (waiting for identification)
    #[serde_as(as = "DurationSeconds<u64>")]
    pub ti: Duration,
    /// Timer for response (waiting for heartbeat or data)
    #[serde_as(as = "DurationSeconds<u64>")]
    pub tr: Duration,
    /// Timer for sending heartbeats, if no other data is sent
    #[serde_as(as = "DurationSeconds<u64>")]
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
impl ConnectionConfig {
    /// Validates the connection configuration
    ///
    /// # Errors
    ///
    /// Returns errors in case tr is smaller than ts or no `remote_addresses` are defined for a
    /// client connection.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.tr < self.ts {
            bail!("Tr must be larger than Ts");
        }
        if self.role == Role::Client && self.remote_addresses.is_empty() {
            bail!("At least one address must be specified in remote_addresses");
        }

        Ok(())
    }
}

/// Global FMTP configuration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Config {
    /// Server bind address for all server connections
    pub bind_address: Option<SocketAddr>,
    /// Map of connection configurations by connection ID
    pub connections: HashMap<String, ConnectionConfig>,
    /// Initial connection timeout before the remote partner has identified itself and the
    /// connection config can be matched to the connection. Only used by servers.
    pub server_ti: Option<Duration>,
}

impl Config {
    /// Load this configuration from a TOML file, see an example in top-level directory of the
    /// repository.
    ///
    /// # Errors
    ///
    /// Returns errors for reading the file, deserialising and validating.
    pub fn load_toml(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(&read_to_string(path)?)?;

        config.validate()
    }

    /// Validates the configuration
    ///
    /// # Errors
    ///
    /// Returns errors in case there are no connections defined, server connections exist but
    /// no `bind_address` is specified, or any of the connections fails [`ConnectionConfig::validate`]
    pub fn validate(mut self) -> anyhow::Result<Self> {
        if let Some(max_ti) = self
            .connections
            .iter()
            .filter_map(|(_id, c)| (c.role == Role::Server).then_some(c.ti))
            .max()
        {
            if self.bind_address.is_none() {
                bail!("Configurations including connections need a bind_address");
            }

            self.server_ti = self.server_ti.or(Some(max_ti));
        }

        if self.connections.is_empty() {
            bail!("At least one connection must be specified");
        }

        let conn_errs = self
            .connections
            .iter()
            .filter_map(|(id, c)| c.validate().err().map(|e| format!("{id}: {e}")))
            .collect::<Vec<_>>();
        if !conn_errs.is_empty() {
            bail!(conn_errs.join("\n"))
        }

        Ok(self)
    }
}

// TODO
// new: validate (or auto-fill) server_ti and bind_address for servers
// bind_address -> ConnectionConfig?
