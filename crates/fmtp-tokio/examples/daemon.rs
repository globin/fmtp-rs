//! Example FMTP daemon using fmtp-tokio.
//!
//! This example demonstrates how to configure and run an FMTP daemon.

use std::time::Duration;

use fmtp_core::Config;
use fmtp_core::ConnectionConfig;
use fmtp_core::FmtpIdentifier;
use fmtp_core::Role;
use fmtp_core::Target;

use fmtp_tokio::Daemon;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::ERROR.into())
                .with_env_var("FMTP_LOG")
                .from_env_lossy(),
        )
        .init();

    let config = Config {
        connections: [
            (
                "fmtp".to_string(),
                ConnectionConfig {
                    connect_retry_timer: None,
                    role: Role::Server,
                    initial_target: Target::DataReady,
                    remote_addresses: vec!["127.0.0.1:8500".parse().unwrap()],
                    local_id: FmtpIdentifier::new("SERVER")?,
                    remote_id: FmtpIdentifier::new(b"CLIENT")?,
                    ti: Duration::from_secs(30),
                    ts: Duration::from_secs(15),
                    tr: Duration::from_secs(40),
                },
            ),
            (
                "fmtp-client".to_string(),
                ConnectionConfig {
                    connect_retry_timer: Some(Duration::from_secs(5)),
                    role: Role::Client,
                    initial_target: Target::DataReady,
                    remote_addresses: vec!["127.0.0.1:8500".parse().unwrap()],
                    remote_id: FmtpIdentifier::new(b"SERVER")?,
                    local_id: FmtpIdentifier::new(b"CLIENT")?,
                    ti: Duration::from_secs(30),
                    ts: Duration::from_secs(15),
                    tr: Duration::from_secs(40),
                },
            ),
        ]
        .into(),
        bind_address: Some("127.0.0.1:8500".parse().unwrap()),
        server_ti: Some(Duration::from_secs(30)),
    };

    let daemon = Daemon::new(config);

    daemon.run().await?.join_all().await;

    Ok(())
}
