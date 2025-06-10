use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::bail;
use fmtp_core::{Config, FmtpMessage, Role, State, UserCommand};
use tokio::{
    net::TcpListener,
    spawn,
    sync::{
        Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    task::JoinSet,
    time::sleep,
};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{Connection, connection::ConnectionEvent};

#[derive(Debug)]
pub struct ConnectionState {
    pub state: State,
    pub remote_addr: Option<SocketAddr>,
    pub command_tx: Sender<UserCommand>,
    pub msg_rx: Receiver<FmtpMessage>,
}

pub struct Server {
    config: Config,
    connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
}

impl Server {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn run(&self) -> anyhow::Result<JoinSet<anyhow::Result<()>>> {
        let mut handles = JoinSet::new();
        let connections = self.connections.clone();
        spawn(async move {
            loop {
                sleep(Duration::from_secs(15)).await;
                let connections = connections.lock().await;
                info!("connection state: {:?}", *connections);
            }
        });

        if let Some(bind_address) = self.config.bind_address {
            let socket = TcpListener::bind(bind_address).await?;
            info!("Binding to address: {}", bind_address);
            self.run_server_loop(socket, &mut handles);
        } else if let Some(name) = self
            .config
            .connections
            .iter()
            .find_map(|(name, c)| (c.role == Role::Server).then_some(name))
        {
            bail!("Binding as server for {name}, but no bind address specified");
        }

        self.run_client_loop(&mut handles).await?;

        Ok(handles)
    }

    fn run_server_loop(&self, socket: TcpListener, handles: &mut JoinSet<anyhow::Result<()>>) {
        let connections = self.connections.clone();
        let conn_config = self.config.clone();
        handles.spawn(async move {
            loop {
                info!("Waiting for FMTP connection");
                let connections = connections.clone();
                let conn_config = conn_config.clone();
                let (socket, remote_addr) = socket.accept().await?;

                let (ev_tx, mut ev_rx) = channel(1024);
                let (command_tx, command_rx) = channel(1024);
                spawn(async move {
                    let mut conn = Connection::new(conn_config, None, ev_tx, command_rx)?;
                    conn.run_server(socket).await?;
                    Ok::<_, anyhow::Error>(())
                });

                spawn(async move {
                    let mut id = Uuid::new_v4().to_string();
                    let (msg_tx, msg_rx) = channel(1024);
                    connections.lock().await.insert(
                        id.clone(),
                        ConnectionState {
                            state: State::Idle {},
                            remote_addr: Some(remote_addr),
                            command_tx: command_tx.clone(),
                            msg_rx,
                        },
                    );

                    loop {
                        if let Some(ev) = ev_rx.recv().await {
                            match ev {
                                ConnectionEvent::StateChanged(state) => {
                                    if let Some(c) = connections.lock().await.get_mut(&id) {
                                        debug!("{id}: state changed to {state:?}");
                                        c.state = state;
                                    } else {
                                        bail!("{id}: no connection found to set state: {state:?}")
                                    }
                                }
                                ConnectionEvent::ConnectionIdDetermined(determined_id) => {
                                    debug!("{determined_id}: id determined");
                                    let connection = { connections.lock().await.remove(&id) };
                                    if let Some(c) = connection {
                                        connections.lock().await.insert(determined_id.clone(), c);
                                        id = determined_id;
                                    } else {
                                        bail!(
                                            "connection state no longer found for {determined_id}"
                                        );
                                    }
                                }
                                ConnectionEvent::DataReceived(msg) => {
                                    info!("{id}: msg received, {msg}");
                                    msg_tx.send(msg).await?;
                                }
                            }
                        } else {
                            debug!("{id}: connection closed");
                            connections.lock().await.remove(&id);
                            return Ok::<_, anyhow::Error>(());
                        }
                    }
                });
            }
        });
    }

    async fn run_client_loop(
        &self,
        handles: &mut JoinSet<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        for (id, _) in self
            .config
            .connections
            .iter()
            .filter(|(_, c)| c.role == Role::Client)
        {
            let (msg_tx, msg_rx) = channel(1024);
            let (ev_tx, mut ev_rx) = channel(1024);
            let (command_tx, command_rx) = channel(1024);
            let mut client =
                Connection::new(self.config.clone(), Some(id.clone()), ev_tx, command_rx)?;
            let connections = self.connections.clone();

            connections.lock().await.insert(
                id.clone(),
                ConnectionState {
                    state: State::Idle {},
                    remote_addr: None,
                    command_tx: command_tx.clone(),
                    msg_rx,
                },
            );

            handles.spawn(async move { client.run(None).await });

            let id = id.clone();
            spawn(async move {
                loop {
                    // FIXME set remote_addr
                    if let Some(ev) = ev_rx.recv().await {
                        match ev {
                            ConnectionEvent::StateChanged(state) => {
                                if let Some(c) = connections.lock().await.get_mut(&*id) {
                                    debug!("{id}: state changed to {state:?}");
                                    c.state = state;
                                } else {
                                    bail!("{id}: no connection found to set state: {state:?}")
                                }
                            }
                            ConnectionEvent::ConnectionIdDetermined(determined_id) => {
                                debug!("{id}: connection ID determined, {determined_id}");
                            }
                            ConnectionEvent::DataReceived(msg) => {
                                info!("{id}: msg received, {msg}");
                                msg_tx.send(msg).await?;
                            }
                        }
                    } else {
                        debug!("{id}: connection closed");
                        connections.lock().await.remove(&id);
                        return Ok::<_, anyhow::Error>(());
                    }
                }
            });
        }

        Ok(())
    }

    /// Creates a new reference to the [`ConnectionState`] [`HashMap`]
    #[must_use]
    pub fn connections(&self) -> Arc<Mutex<HashMap<String, ConnectionState>>> {
        self.connections.clone()
    }
}
