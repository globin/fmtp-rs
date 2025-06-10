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

#[derive(Debug)]
pub enum ConnectionEvent {
    StateChanged(State),
    ConnectionIdDetermined(String),
    DataReceived(FmtpMessage),
}

#[derive(Debug)]
pub struct Connection {
    inner: CoreConnection,
    ctx: ConnectionContext,
    tx: Sender<ConnectionEvent>,
    rx: Receiver<UserCommand>,
}
impl Connection {
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
                    &Event::UserCommand(UserCommand::Setup {
                        id: connection.to_string(),
                    }),
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

    pub async fn run_server(&mut self, socket: TcpStream) -> anyhow::Result<()> {
        self.inner.handle_with_context(
            &Event::RSetup {
                now: Instant::now().into(),
            },
            &mut self.ctx,
        );
        self.run(Some(socket)).await
    }

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
            self.inner
                .handle_with_context(&Event::UserCommand(cmd), &mut self.ctx);
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
    pub async fn run(&mut self, mut socket: Option<TcpStream>) -> anyhow::Result<()> {
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
                                self.inner.handle_with_context(&Event::UserCommand(UserCommand::Startup {
                                    now: Instant::now().into(),
                                }), &mut self.ctx);
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
