use std::net::SocketAddr;

use anyhow::anyhow;
use fmtp_core::{
    Config, Connection as CoreConnection, ConnectionContext, Event, FmtpMessage, FmtpPacket, Role,
    State, Target, Ti, Tr, Ts, UserCommand,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    select,
    sync::mpsc::{Receiver, Sender},
    time::{Instant, sleep, sleep_until},
};
use tracing::{debug, error};

/// Events emitted by an FMTP connection
///
/// These events represent state changes, connection establishment,
/// and data reception in an FMTP connection.
#[derive(Debug)]
pub enum ConnectionEvent {
    /// The connection's state has changed
    StateChanged(State),
    /// The connection's identifier has been determined
    ConnectionIdDetermined(String),
    /// A message has been received from the remote peer
    DataReceived(FmtpMessage),
}

/// An asynchronous FMTP connection
///
/// This struct represents a single FMTP connection, which can operate in either client
/// or server mode. It handles the protocol state machine, connection establishment,
/// and message exchange.
///
/// # Examples
///
/// ```no_run
/// use fmtp_tokio::Connection;
/// use fmtp_core::{Config, FmtpIdentifier, ConnectionConfig, Role, Target, UserCommand};
/// use std::time::Duration;
/// use tokio::sync::mpsc::channel;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = Config {
///         connections: [
///             (
///                 "fmtp-client".to_string(),
///                 ConnectionConfig {
///                     connect_retry_timer: None,
///                     role: Role::Client,
///                     initial_target: Target::DataReady,
///                     remote_addresses: vec!["127.0.0.1:8500".parse().unwrap()],
///                     remote_id: FmtpIdentifier::new(b"SERVER")?,
///                     local_id: FmtpIdentifier::new(b"CLIENT")?,
///                     ti: Duration::from_secs(30),
///                     ts: Duration::from_secs(15),
///                     tr: Duration::from_secs(40),
///                 },
///             ),
///         ]
///         .into(),
///         bind_address: Some("127.0.0.1:8500".parse().unwrap()),
///         server_ti: Some(Duration::from_secs(30)),
///     };
///
///     let (ev_tx, mut ev_rx) = channel(1024);
///     let (cmd_tx, cmd_rx) = channel(1024);
///
///     let mut conn = Connection::new(config, Some("fmtp-client".to_string()), ev_tx, cmd_rx)?;
///     conn.run_client().await?;
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct Connection {
    inner: CoreConnection,
    ctx: ConnectionContext,
    tx: Sender<ConnectionEvent>,
    rx: Receiver<UserCommand>,
}

impl Connection {
    /// Creates a new FMTP connection
    ///
    /// # Arguments
    ///
    /// * `cfg` - The FMTP configuration
    /// * `connection_id` - Optional identifier for this connection
    /// * `tx` - Channel sender for connection events
    /// * `rx` - Channel receiver for user commands
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be created with the given configuration
    pub fn new(
        cfg: Config,
        connection_id: Option<String>,
        tx: Sender<ConnectionEvent>,
        rx: Receiver<UserCommand>,
    ) -> anyhow::Result<Self> {
        let mut ctx = ConnectionContext::default();
        Ok(Self {
            inner: CoreConnection::new(cfg, &mut ctx, connection_id),
            ctx,
            tx,
            rx,
        })
    }

    /// Attempts to establish a TCP connection to a remote FMTP peer
    ///
    /// # Arguments
    ///
    /// * `connection` - The identifier of the connection configuration to use
    ///
    /// # Returns
    ///
    /// Returns a tuple of the remote socket address and the established TCP stream
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established with any of the configured addresses
    pub async fn connect(&mut self, connection: &str) -> anyhow::Result<(SocketAddr, TcpStream)> {
        if let Some(remote_addresses) = self
            .inner
            .config
            .connections
            .get(connection)
            .map(|cc| cc.remote_addresses.clone())
        {
            for addr in &remote_addresses {
                self.inner.handle_with_context(
                    &Event::LSetup {
                        id: connection.to_string(),
                    },
                    &mut self.ctx,
                );
                debug!("Connecting socket: {addr}");

                let socket = match TcpStream::connect(addr).await {
                    Ok(socket) => socket,
                    Err(e) => {
                        debug!("Could not connect to {addr}: {e}");
                        self.inner
                            .handle_with_context(&Event::RDisconnect, &mut self.ctx);
                        continue;
                    }
                };
                self.inner.handle_with_context(
                    &Event::RSetup {
                        now: Instant::now().into(),
                    },
                    &mut self.ctx,
                );

                return Ok((*addr, socket));
            }
        }

        Err(anyhow!("Could not connect to any address: {connection}"))
    }

    /// Runs the connection in server mode with an established TCP stream
    ///
    /// # Arguments
    ///
    /// * `socket` - The TCP stream for an accepted connection
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails during operation
    pub async fn run_server(&mut self, socket: TcpStream) -> anyhow::Result<()> {
        self.inner.handle_with_context(
            &Event::RSetup {
                now: Instant::now().into(),
            },
            &mut self.ctx,
        );
        self.run(Some(socket)).await
    }

    /// Runs the connection in client mode
    ///
    /// This method will attempt to establish a connection to the remote peer
    /// and then handle the FMTP protocol exchange.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails during operation
    pub async fn run_client(&mut self) -> anyhow::Result<()> {
        self.run(None).await
    }

    async fn send_packet(&mut self, socket: Option<&mut TcpStream>, packet: FmtpPacket) {
        // FIXME check valid state to transmit type of packet
        debug!("{:?}: transmitting packet: {packet}", self.inner.id);
        // unwrap is safe due to check in select branch above
        if let Err(e) = socket.unwrap().write(&packet.into_bytes()).await {
            error!("disconnected: {e}");
            self.inner
                .handle_with_context(&Event::RDisconnect, &mut self.ctx);
        }
    }

    fn command_received(&mut self, command: Option<UserCommand>) -> bool {
        if let Some(cmd) = command {
            debug!("{:?}: handling mt-cmd: {cmd:?}", self.inner.id);
            let event = match cmd {
                UserCommand::Data { msg } => Event::LData {
                    now: Instant::now().into(),
                    msg,
                },
                UserCommand::Shutdown => Event::LShutdown {
                    now: Instant::now().into(),
                },
                UserCommand::Startup => Event::LStartup {
                    now: Instant::now().into(),
                },
                UserCommand::Setup { id } => Event::LSetup { id },
                UserCommand::Disconnect => Event::LDisconnect,
            };
            self.inner.handle_with_context(&event, &mut self.ctx);
            false
        } else {
            error!("MT-User command channel closed");
            true
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "needs refactoring, but hard to split the select"
    )]
    async fn run(&mut self, mut socket: Option<TcpStream>) -> anyhow::Result<()> {
        let mut buffer = [0; 10240];
        let mut state = State::Idle {};
        let mut id = None;
        'connection: loop {
            select! {
                // This orders handling of events as follows:
                // 1. transmit all queued packets over TCP/IP
                // 2. receive new events from MT-user (via channel rx)
                // 3. transmit received FMTP messages (to channel tx)
                // 4. handle state machine events (timers, react to FMTP packets)
                // 5. enqueue new incoming packets from TCP connection
                biased;

                packet = self.ctx.transmit_future(), if socket.is_some() => {
                    self.send_packet(socket.as_mut(), packet).await;
                }

                received_val = self.rx.recv() => {
                    if self.command_received(received_val) {
                        break 'connection;
                    }
                }

                msg = self.ctx.receive_future() => {
                    debug!("received message: {msg}");
                    if let Err(e) = self.tx.send(ConnectionEvent::DataReceived(msg)).await {
                        error!("MT-User receiving channel closed: {e}");
                        break 'connection;
                    }
                }

                res = async {
                    match self.inner.state() {
                        State::Idle {} => {
                            return Ok(true);
                        }
                        State::ConnectionPending {} => {
                            return Err(anyhow!("Unexpectedly remained in intermediate state CONNECTION_PENDING"));
                        }
                        State::IdPending { ti: Ti(ti) } |
                            State::AcceptPending { ti: Ti(ti) } => {
                                sleep_until((*ti).into()).await;
                                debug!("ti timed out");
                                self.inner.handle_with_context(&Event::TiTimeout {}, &mut self.ctx);
                        }
                        State::Ready { tr: Tr(tr) } => {
                            if self.inner.target == Target::DataReady {
                                self.inner.handle_with_context(&Event::LStartup {
                                    now: Instant::now().into(),
                                }, &mut self.ctx);
                            } else {
                                sleep_until((*tr).into()).await;
                            }
                        }
                        State::AssociationPending { tr: Tr(tr) } => {
                            sleep_until((*tr).into()).await;
                            debug!("tr timed out");
                            self.inner.handle_with_context(&Event::TrTimeout { now: Instant::now().into() }, &mut self.ctx);
                        }
                        State::DataReady {
                            tr: Tr(tr),
                            ts: Ts(ts),
                        } => {
                            select! {
                                () =  sleep_until((*tr).into()) => {
                                    debug!("tr timed out");
                                    self.inner.handle_with_context(&Event::TrTimeout { now: Instant::now().into() }, &mut self.ctx);
                                }
                                () = sleep_until((*ts).into()) => {
                                    debug!("ts timed out");
                                    self.inner.handle_with_context(&Event::TsTimeout { now: Instant::now().into() }, &mut self.ctx);
                                }
                            }
                        }
                        State::Error {} => {
                            return Err(anyhow!("reached state ERROR, terminating"));
                        }
                    }

                    Ok(false)
                } => {
                    match res {
                        Err(e) => {
                            error!("{e}");
                            break 'connection;
                        }
                        Ok(true) => {
                            if let Some(id) = self.inner.id.clone() {
                                if socket.is_none() {
                                    if let Ok((_addr, new_socket)) = self.connect(&id).await {
                                        socket = Some(new_socket);
                                    } else {
                                        debug!("Could not connect to any address");
                                        if let Some(timer) = self.inner.config.connections.get(&*id).and_then(|c|
                                            (c.role == Role::Client).then_some(c.connect_retry_timer).flatten()
                                        ) {
                                            debug!("Reconnecting in {} seconds", timer.as_secs());
                                            sleep(timer).await;
                                        } else {
                                            debug!("No reconnection timer set, not retrying to connect.");
                                            break 'connection;
                                        }
                                    }
                                } else if let Some(timer) = self.inner.config.connections.get(&*id).and_then(|c|
                                    (c.role == Role::Client).then_some(c.connect_retry_timer).flatten()
                                ) {
                                    if let Some(s) = socket.as_mut() {
                                        s.shutdown().await?;
                                    }
                                    debug!("Reconnecting in {} seconds", timer.as_secs());
                                    sleep(timer).await;
                                    if let Ok((_addr, new_socket)) = self.connect(&id).await {
                                        socket = Some(new_socket);
                                    } else {
                                        debug!("Could not reconnect to any address");
                                    }
                                } else {
                                    debug!("No reconnection timer set, disconnecting.");
                                    break 'connection;
                                }
                            } else {
                                debug!("Server connection, disconnecting.");
                                break 'connection;
                            }
                        }
                        Ok(false) => ()
                    }
                }
                received = async { socket.as_mut().unwrap().read(&mut buffer).await }, if socket.is_some() => {
                    match received {
                        Ok(bytes_received) => {
                            self.inner.handle_remote_packet(&buffer, bytes_received, Instant::now().into(), &mut self.ctx)?;
                        }
                        Err(e) => {
                            debug!("{e}");
                            self.inner.handle_with_context(&Event::RDisconnect, &mut self.ctx);
                        }
                    }
                }
            }

            if id.is_none() {
                if let Some(determined_id) = &self.inner.id {
                    debug!("transmitting id change");
                    id = Some(determined_id.clone());
                    self.tx
                        .send(ConnectionEvent::ConnectionIdDetermined(
                            determined_id.clone(),
                        ))
                        .await?;
                }
            }
            if *self.inner.state() != state {
                debug!("transmitting state change");
                state = self.inner.state().clone();
                self.tx
                    .send(ConnectionEvent::StateChanged(state.clone()))
                    .await?;
            }
        }

        debug!("Disconnecting");
        if let Some(mut s) = socket {
            s.shutdown().await?;
        }

        Ok(())
    }
}
