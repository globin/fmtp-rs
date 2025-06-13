use std::io::BufReader;
use std::io::Read as _;
use std::io::stdin;
use std::thread;
use std::time::Duration;

use fmtp_core::Config;
use fmtp_core::ConnectionConfig;
use fmtp_core::FmtpIdentifier;

use fmtp_core::FmtpMessage;
use fmtp_core::Role;
use fmtp_core::Target;
use fmtp_core::UserCommand;
use fmtp_tokio::Connection;

use fmtp_tokio::ConnectionEvent;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::channel;
use tracing::debug;
use tracing::info;
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

    let (ev_tx, mut ev_rx) = channel(1024);
    let (cmd_tx, cmd_rx) = channel(1024);
    let mut client = Connection::new(
        Config {
            connections: [(
                "local".to_string(),
                ConnectionConfig {
                    remote_addresses: vec![
                        "127.0.0.1:8500".parse().unwrap(),
                        "[::1]:8500".parse().unwrap(),
                    ],
                    remote_id: FmtpIdentifier::new("SERVER".as_bytes())?,
                    local_id: FmtpIdentifier::new("CLIENT".as_bytes())?,
                    ti: Duration::from_secs(30),
                    ts: Duration::from_secs(15),
                    tr: Duration::from_secs(40),
                    role: Role::Client,
                    initial_target: Target::DataReady,
                    connect_retry_timer: Some(Duration::from_secs(3)),
                },
            )]
            .into(),
            bind_address: None,
            server_ti: None,
        },
        Some("local".to_string()),
        ev_tx,
        cmd_rx,
    )?;

    tokio::spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
            match ev {
                ConnectionEvent::StateChanged(state) => {
                    info!("State changed: {state:?}");
                }
                ConnectionEvent::ConnectionIdDetermined(id) => {
                    info!("Id determined: {id}");
                }
                ConnectionEvent::DataReceived(msg) => {
                    info!("Message received: {msg}");
                }
            }
        }
    });

    debug!("spawning thread");
    thread::spawn(move || {
        let stdin = stdin();
        let reader = BufReader::new(stdin);
        for byte in reader.bytes() {
            let command = byte?;
            debug!("received {command}");
            let tx = cmd_tx.clone();
            let rt = Runtime::new()?;
            rt.block_on(async move {
                match command {
                    b'a' => {
                        debug!("sending startup");
                        tx.send(UserCommand::Startup).await?;
                    }
                    b's' => {
                        debug!("sending shutdown");
                        tx.send(UserCommand::Shutdown).await?;
                    }
                    b'd' => {
                        debug!("sending disconnect");
                        tx.send(UserCommand::Disconnect {}).await?;
                    }
                    b'o' => {
                        debug!("sending data");
                        tx.send(UserCommand::Data {
                            msg: FmtpMessage::Operational(b"test".into()),
                        })
                        .await?;
                    }
                    _ => debug!("other"),
                }

                Ok::<_, anyhow::Error>(())
            })?;
        }
        debug!("ending thread");

        Ok::<_, anyhow::Error>(())
    });

    client.run(None).await?;

    Ok(())
}
