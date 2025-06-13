use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    time::Instant,
};

use crate::{Config, Event, FmtpMessage, FmtpPacket, RxFut, Target, Ti, Tr, Ts, TxFut};

use statig::{
    Response, StateOrSuperstate,
    prelude::{InitializedStateMachine, IntoStateMachineExt as _},
    state_machine,
};
use tracing::{debug, error, trace, warn};

/// Context for managing packet and message queues in an FMTP connection.
///
/// This type maintains two queues:
/// - A send queue for outgoing FMTP packets
/// - A receive queue for incoming FMTP messages
///
/// The queues are used to buffer data between the state machine and the transport layer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConnectionContext {
    send_queue: VecDeque<FmtpPacket>,
    recv_queue: VecDeque<FmtpMessage>,
}

impl ConnectionContext {
    /// Polls for the next packet to transmit.
    ///
    /// # Returns
    /// * `Some(FmtpPacket)` if there's a packet ready to send
    /// * `None` if the send queue is empty
    pub fn poll_transmit(&mut self) -> Option<FmtpPacket> {
        self.send_queue.pop_front()
    }
    /// Creates a future that completes when a packet is ready to transmit.
    pub fn transmit_future(&mut self) -> TxFut {
        TxFut(self.poll_transmit())
    }
    /// Polls for the next received message.
    ///
    /// # Returns
    /// * `Some(FmtpMessage)` if there's a message ready to process
    /// * `None` if the receive queue is empty
    pub fn poll_receive(&mut self) -> Option<FmtpMessage> {
        self.recv_queue.pop_front()
    }
    /// Creates a future that completes when a message is ready to receive.
    pub fn receive_future(&mut self) -> RxFut {
        RxFut(self.poll_receive())
    }
}

/// An FMTP connection state machine.
///
/// This type implements the FMTP protocol state machine as defined in the
/// EUROCONTROL FMTP v2.0 specification. It handles:
/// - Connection establishment and teardown
/// - Identification and handshaking
/// - Data transfer and heartbeat mechanisms
/// - Timer management and error handling
#[derive(Debug, Clone)]
pub struct Connection(InitializedStateMachine<ConnectionState>);
impl Deref for Connection {
    type Target = InitializedStateMachine<ConnectionState>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for Connection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Connection {
    pub fn new(config: Config, ctx: &mut ConnectionContext, id: Option<String>) -> Self {
        let sm = ConnectionState {
            config,
            server: false,
            id,
            target: Target::Idle,
        }
        .uninitialized_state_machine()
        .init_with_context(ctx);
        Self(sm)
    }

    pub fn handle_remote_packet(
        &mut self,
        bytes: &[u8],
        received: usize,
        now: Instant,
        ctx: &mut ConnectionContext,
    ) -> anyhow::Result<()> {
        let remote_id_packet = FmtpPacket::from_bytes(bytes)?;
        debug!("received remote packet ({received} bytes): {remote_id_packet}");
        let event = Event::from_incoming_packet(
            received,
            remote_id_packet,
            now,
            &self.config,
            self.id.as_deref(),
        )?;

        self.handle_with_context(&event, ctx);

        Ok::<(), anyhow::Error>(())
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionState {
    pub config: Config,
    pub server: bool,
    pub target: Target,
    pub id: Option<String>,
}

/// The core Flight Message Transfer Protocol (FMTP) state machine implementation.
/// This implements the state machine as specified in the EUROCONTROL FMTP v2.0 specification.
///
/// The state machine has the following states:
/// - IDLE: Initial state, no connection established
/// - CONNECTION_PENDING: Connection request sent, waiting for TCP connection establishment
/// - ID_PENDING: Connection established, waiting for identification
/// - ACCEPT_PENDING: Identification received, waiting for accept/reject
/// - READY: Connection established and identified but not ready to transfer operational data
/// - ASSOCIATION_PENDING: Association request sent, waiting for response
/// - DATA_READY: Data transfer association established and active
/// - ERROR: Error state, connection will be terminated
///
/// The state machine transitions based on:
/// - Local user commands ([`UserCommand`])
/// - Remote events (incoming packets)
/// - Timer expiry events
///
/// Key timers:
/// - Ti: Identification timeout
/// - Tr: Response timeout
/// - Ts: Send heartbeat timeout
#[state_machine(
    initial = "State::idle()",
    before_dispatch = "Self::before_dispatch",
    after_dispatch = "Self::after_dispatch",
    before_transition = "Self::before_transition",
    after_transition = "Self::after_transition",
    state(derive(Clone, Debug, PartialEq, Eq)),
    superstate(derive(Clone, Debug))
)]
impl ConnectionState {
    #[expect(clippy::needless_pass_by_value, reason = "due to macro")]
    fn before_dispatch(&mut self, state: StateOrSuperstate<State, Superstate>, event: &Event) {
        match event {
            Event::LShutdown { .. } => self.target = Target::Ready,
            Event::LStartup { .. } => self.target = Target::DataReady,
            Event::LDisconnect => self.target = Target::Idle,
            // FIXME include target in event?
            // Event::UserCommand(UserCommand::Setup { id }) => self.target = Target::Ready,
            _ => (),
        }
        trace!(
            "before dispatching `{:?}` to `{:?}`, target: {:?}",
            event, state, self.target
        );
    }
    #[expect(clippy::needless_pass_by_value, reason = "due to macro")]
    fn after_dispatch(&mut self, state: StateOrSuperstate<State, Superstate>, event: &Event) {
        trace!(
            "{:?}: after dispatching `{:?}` to `{:?}`",
            self.id, event, state
        );
    }
    fn before_transition(&mut self, next: &State, prev: &State) {
        trace!(
            "{:?}: before transitioning from `{:?}` to `{:?}`",
            self.id, prev, next
        );
    }
    fn after_transition(&mut self, prev: &State, next: &State) {
        trace!(
            "{:?}: after transitioning from `{:?}` to `{:?}`",
            self.id, prev, next
        );
    }

    /// IDLE state handler.
    /// This is the initial state of the connection where no communication is established.
    ///
    /// From this state:
    /// - Client can initiate connection with Setup command
    /// - Server can receive RSetup event
    /// - Disconnect and RDisconnect remain in IDLE
    /// - All other events are errors
    #[state]
    fn idle(&mut self, event: &Event) -> Response<State> {
        match event {
            Event::RSetup { now } => {
                let Some(server_ti) = self.config.server_ti else {
                    error!("no server_ti, but server connection");
                    return Response::Transition(State::error());
                };
                self.server = true;
                Response::Transition(State::id_pending(Ti(*now + server_ti)))
            }
            Event::RDisconnect | Event::LDisconnect => Response::Handled,
            Event::LSetup { id } => {
                let Some(config) = self.config.connections.get(id) else {
                    error!("Invalid id for connection: {id}");
                    return Response::Transition(State::error());
                };
                self.id = Some(id.clone());
                self.server = false;
                self.target = config.initial_target;
                Response::Transition(State::connection_pending())
            }
            Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::RData { .. }
            | Event::RHeartbeat { .. }
            | Event::RShutdown { .. }
            | Event::TiTimeout
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::RStartup { .. }
            | Event::LData { .. }
            | Event::LShutdown { .. }
            | Event::LStartup { .. } => {
                error!("Unexpected Event {event:?} in IDLE");
                Response::Transition(State::error())
            }
        }
    }

    /// CONNECTION_PENDING state handler.
    /// The local system has sent a connection request and is waiting for acknowledgment.
    ///
    /// From this state:
    /// - RSetup -> ID_PENDING (send handshake)
    /// - RDisconnect/Disconnect -> IDLE
    /// - All other events are errors
    #[state]
    fn connection_pending(
        &mut self,
        context: &mut ConnectionContext,
        event: &Event,
    ) -> Response<State> {
        let Some(config) = self
            .id
            .as_ref()
            .and_then(|c| self.config.connections.get(c))
        else {
            return Response::Transition(State::idle());
        };
        match event {
            Event::RSetup { now } => {
                context
                    .send_queue
                    .push_back(FmtpPacket::handshake(&config.local_id, &config.remote_id));
                self.target = config.initial_target;
                Response::Transition(State::id_pending(Ti(*now + config.ti)))
            }
            Event::RDisconnect | Event::LDisconnect => Response::Transition(State::idle()),
            Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::RData { .. }
            | Event::RHeartbeat { .. }
            | Event::RShutdown { .. }
            | Event::TiTimeout
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::RStartup { .. }
            | Event::LSetup { .. }
            | Event::LData { .. }
            | Event::LShutdown { .. }
            | Event::LStartup { .. } => {
                error!("Unexpected Event {event:?} in CONNECTION_PENDING");
                Response::Transition(State::error())
            }
        }
    }

    /// ID_PENDING state handler.
    /// Connection established, waiting for identification from remote system.
    ///
    /// From this state:
    /// - Server: RIdValid -> ACCEPT_PENDING
    /// - Client: RIdValid -> send accept
    /// - Timer Ti expiry -> IDLE
    /// - RDisconnect/Disconnect -> IDLE
    #[state]
    fn id_pending(
        &mut self,
        context: &mut ConnectionContext,
        event: &Event,
        #[expect(unused_variables, reason = "needed to store in state")] ti: &Ti,
    ) -> Response<State> {
        match event {
            Event::RIdValid { now, id } if self.server => {
                self.id = Some(id.clone());
                let Some(config) = self
                    .id
                    .as_ref()
                    .and_then(|c| self.config.connections.get(c))
                else {
                    return Response::Transition(State::idle());
                };
                // as server we close the connection if we go to Idle
                // so we only reach this transition on `accept` and
                // it is safe to set the target to initial target
                self.target = config.initial_target;

                context
                    .send_queue
                    .push_back(FmtpPacket::handshake(&config.local_id, &config.remote_id));
                Response::Transition(State::accept_pending(Ti(*now + config.ti)))
            }
            Event::RIdValid { now, id: _ } => {
                let Some(config) = self
                    .id
                    .as_ref()
                    .and_then(|c| self.config.connections.get(c))
                else {
                    return Response::Transition(State::idle());
                };
                context.send_queue.push_back(FmtpPacket::accept());
                Response::Transition(State::ready(Tr(*now + config.tr)))
            }
            Event::RIdInvalid => {
                context.send_queue.push_back(FmtpPacket::reject());
                Response::Transition(State::idle())
            }
            Event::RDisconnect
            | Event::RReject
            | Event::RAccept { .. }
            | Event::RData { .. }
            | Event::RHeartbeat { .. }
            | Event::RShutdown { .. }
            | Event::RStartup { .. }
            | Event::TiTimeout
            | Event::LDisconnect => Response::Transition(State::idle()),
            Event::RSetup { .. }
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::LSetup { .. }
            | Event::LShutdown { .. }
            | Event::LStartup { .. }
            | Event::LData { .. } => {
                error!("Unexpected Event {event:?} in ID_PENDING");
                Response::Transition(State::error())
            }
        }
    }

    /// ACCEPT_PENDING state handler.
    /// Identification successful, waiting for accept/reject response.
    ///
    /// From this state:
    /// - RAccept -> READY
    /// - RReject/Ti timeout -> IDLE
    /// - RDisconnect/Disconnect -> IDLE
    #[state]
    fn accept_pending(
        &self,
        event: &Event,
        #[expect(unused_variables, reason = "needed to store in state")] ti: &Ti,
    ) -> Response<State> {
        let Some(config) = self
            .id
            .as_ref()
            .and_then(|c| self.config.connections.get(c))
        else {
            return Response::Transition(State::idle());
        };
        match event {
            Event::RAccept { now } => Response::Transition(State::ready(Tr(*now + config.tr))),
            Event::RDisconnect
            | Event::RReject
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::RData { .. }
            | Event::RHeartbeat { .. }
            | Event::RShutdown { .. }
            | Event::RStartup { .. }
            | Event::TiTimeout
            | Event::LDisconnect => Response::Transition(State::idle()),
            Event::RSetup { .. }
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::LSetup { .. }
            | Event::LShutdown { .. }
            | Event::LStartup { .. }
            | Event::LData { .. } => {
                error!("Unexpected Event {event:?} in ID_PENDING");
                Response::Transition(State::error())
            }
        }
    }

    /// READY state handler.
    /// Connection established and identified, ready for data transfer setup.
    ///
    /// From this state:
    /// - Startup -> ASSOCIATION_PENDING
    /// - RDisconnect/Disconnect -> IDLE
    /// - RShutdown/RHeartbeat -> Stay in READY
    #[state]
    fn ready(
        &mut self,
        context: &mut ConnectionContext,
        event: &Event,
        #[expect(unused_variables, reason = "needed to store in state")] tr: &Tr,
    ) -> Response<State> {
        let Some(config) = self
            .id
            .as_ref()
            .and_then(|c| self.config.connections.get(c))
        else {
            return Response::Transition(State::idle());
        };
        match event {
            Event::LStartup { now } => {
                context.send_queue.push_back(FmtpPacket::startup());
                Response::Transition(State::association_pending(Tr(*now + config.tr)))
            }
            Event::RDisconnect | Event::LDisconnect => Response::Transition(State::idle()),
            Event::RShutdown { .. } | Event::RHeartbeat { .. } => Response::Handled,
            Event::RSetup { .. }
            | Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::RData { .. }
            | Event::TiTimeout
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::RStartup { .. }
            | Event::LSetup { .. }
            | Event::LData { .. }
            | Event::LShutdown { .. } => {
                error!("Unexpected Event {event:?} in READY");
                Response::Transition(State::error())
            }
        }
    }

    /// ASSOCIATION_PENDING state handler.
    /// Data transfer association requested, waiting for acknowledgment.
    ///
    /// From this state:
    /// - RStartup -> DATA_READY
    /// - RShutdown -> READY
    /// - RDisconnect/Disconnect -> IDLE
    /// - Tr timeout -> retry association
    #[state]
    fn association_pending(
        &mut self,
        context: &mut ConnectionContext,
        event: &Event,
        #[expect(unused_variables, reason = "needed to store in state")] tr: &Tr,
    ) -> Response<State> {
        let Some(config) = self
            .id
            .as_ref()
            .and_then(|c| self.config.connections.get(c))
        else {
            return Response::Transition(State::idle());
        };
        match event {
            Event::RStartup { now } => {
                context.send_queue.push_back(FmtpPacket::startup());
                Response::Transition(State::data_ready(
                    Tr(*now + config.tr),
                    Ts(*now + config.ts),
                ))
            }
            // restart timer with warning
            Event::TrTimeout { now } => {
                warn!("Tr timed out, restarting association");
                context.send_queue.push_back(FmtpPacket::startup());
                Response::Transition(State::association_pending(Tr(*now + config.tr)))
            }
            Event::RDisconnect => Response::Transition(State::idle()),
            Event::LDisconnect => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::idle())
            }
            Event::LShutdown { now } => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::ready(Tr(*now + config.tr)))
            }
            Event::RSetup { .. }
            | Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::RData { .. }
            | Event::RHeartbeat { .. }
            | Event::RShutdown { .. }
            | Event::TiTimeout
            | Event::TsTimeout { .. }
            | Event::LSetup { .. }
            | Event::LData { .. }
            | Event::LStartup { .. } => {
                error!("Unexpected Event {event:?} in ASSOCIATION_PENDING");
                Response::Transition(State::error())
            }
        }
    }

    /// DATA_READY state handler.
    /// Data transfer association established, can send/receive data.
    ///
    /// From this state:
    /// - RData/Data command -> Stay in DATA_READY, process data
    /// - Ts timeout -> Send heartbeat, stay in DATA_READY
    /// - RShutdown -> ASSOCIATION_PENDING
    /// - RShutdown -> READY
    /// - RDisconnect/Disconnect -> IDLE
    #[state]
    fn data_ready(
        &mut self,
        context: &mut ConnectionContext,
        event: &Event,
        tr: &Tr,
        ts: &Ts,
    ) -> Response<State> {
        let Some(config) = self
            .id
            .as_ref()
            .and_then(|c| self.config.connections.get(c))
        else {
            return Response::Transition(State::idle());
        };
        match event {
            Event::TsTimeout { now } => {
                context.send_queue.push_back(FmtpPacket::heartbeat());
                Response::Transition(State::data_ready(*tr, Ts(*now + config.ts)))
            }
            Event::TrTimeout { .. } | Event::RDisconnect | Event::LDisconnect => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::idle())
            }
            Event::RData { now, msg } => {
                context.recv_queue.push_back(msg.clone());
                Response::Transition(State::data_ready(Tr(*now + config.tr), *ts))
            }
            Event::RHeartbeat { now } | Event::RStartup { now } => {
                Response::Transition(State::data_ready(Tr(*now + config.tr), *ts))
            }
            Event::RShutdown { now } => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::association_pending(Tr(*now + config.tr)))
            }
            Event::LData { now, msg } => {
                context
                    .send_queue
                    .push_back(FmtpPacket::from_msg(msg.clone()));
                Response::Transition(State::data_ready(*tr, Ts(*now + config.ts)))
            }
            Event::LShutdown { now } => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::ready(Tr(*now + config.tr)))
            }
            Event::RSetup { .. }
            | Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::TiTimeout
            | Event::LSetup { .. }
            | Event::LStartup { .. } => {
                error!("Unexpected Event {event:?} in DATA_READY");
                Response::Transition(State::error())
            }
        }
    }

    /// ERROR state handler.
    /// Terminal error state, connection will be terminated.
    ///
    /// No transitions out of this state are possible.
    #[state]
    fn error() -> Response<State> {
        Response::Handled
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Config, Connection, ConnectionConfig, ConnectionContext, Event, FmtpIdentifier,
        FmtpMessage, FmtpPacket, FmtpType, Role, State, Target, Ti, Tr, Ts,
    };
    use std::{
        collections::HashMap,
        time::{Duration, Instant},
    };

    #[test]
    fn test_initial_state_is_idle() {
        let config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };
        let mut ctx = ConnectionContext::default();
        let conn = Connection::new(config, &mut ctx, None);
        assert_eq!(*conn.state(), State::Idle {});
    }

    #[test]
    fn test_client_connection_establishment() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };

        let client_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            remote_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Client,
            initial_target: Target::Ready,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), client_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config, &mut ctx, None);
        let now = Instant::now();

        // Test setup command transitions to CONNECTION_PENDING
        conn.handle_with_context(
            &Event::LSetup {
                id: "test".to_string(),
            },
            &mut ctx,
        );
        assert_eq!(*conn.state(), State::ConnectionPending {});
        assert!(
            ctx.poll_transmit().is_none(),
            "No packets should be queued yet"
        );

        // Test RSetup event transitions to ID_PENDING with correct timer
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::IdPending {
                ti: Ti(now + Duration::from_secs(30))
            }
        );
        let handshake = ctx
            .poll_transmit()
            .expect("Handshake packet should be queued");
        assert_eq!(handshake.header.typ, FmtpType::Identification);

        // Test RIdValid event transitions directly to Ready for client
        conn.handle_with_context(
            &Event::RIdValid {
                now,
                id: "test".to_string(),
            },
            &mut ctx,
        );
        let accept = ctx.poll_transmit().expect("Accept packet should be queued");
        assert_eq!(
            *conn.state(),
            State::Ready {
                tr: Tr(now + Duration::from_secs(40))
            }
        );
        assert!(accept.is_accept());

        // Test received heartbeat is handled in Ready state
        conn.handle_with_context(&Event::RHeartbeat { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::Ready {
                tr: Tr(now + Duration::from_secs(40))
            }
        );
    }

    #[test]
    fn test_server_connection_establishment() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: Some(Duration::from_secs(30)),
        };

        let server_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            remote_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Server,
            initial_target: Target::DataReady,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), server_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config, &mut ctx, None);
        let now = Instant::now();

        // Test RSetup event transitions to ID_PENDING for server
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::IdPending {
                ti: Ti(now + Duration::from_secs(30))
            }
        );
        assert!(
            ctx.poll_transmit().is_none(),
            "No packets should be queued yet"
        );

        // Test RIdValid event for server sends handshake and transitions to AcceptPending
        conn.handle_with_context(
            &Event::RIdValid {
                now,
                id: "test".to_string(),
            },
            &mut ctx,
        );
        assert_eq!(
            *conn.state(),
            State::AcceptPending {
                ti: Ti(now + Duration::from_secs(30))
            }
        );
        let handshake = ctx
            .poll_transmit()
            .expect("Handshake packet should be queued");
        assert_eq!(handshake.header.typ, FmtpType::Identification);

        // Test RAccept transitions to Ready
        conn.handle_with_context(&Event::RAccept { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::Ready {
                tr: Tr(now + Duration::from_secs(40))
            }
        );
    }

    #[test]
    fn test_data_transfer() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };

        let client_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            remote_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Client,
            initial_target: Target::DataReady,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), client_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config.clone(), &mut ctx, Some("test".to_string()));
        let now = Instant::now();

        // Fast forward to Ready state
        conn.handle_with_context(
            &Event::LSetup {
                id: "test".to_string(),
            },
            &mut ctx,
        );
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        conn.handle_with_context(
            &Event::RIdValid {
                now,
                id: "test".to_string(),
            },
            &mut ctx,
        );

        // Clear any queued packets from setup
        while ctx.poll_transmit().is_some() {}

        // Test transition to DATA_READY
        conn.handle_with_context(&Event::LStartup { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::AssociationPending {
                tr: Tr(now + Duration::from_secs(40))
            }
        );
        let startup = ctx
            .poll_transmit()
            .expect("Startup packet should be queued");
        assert!(startup.is_startup());

        // Test RStartup response
        let now_plus_one = now + Duration::from_secs(1); // Advance time
        conn.handle_with_context(&Event::RStartup { now: now_plus_one }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::DataReady {
                tr: Tr(now_plus_one + Duration::from_secs(40)),
                ts: Ts(now_plus_one + Duration::from_secs(15))
            }
        );
        let startup_response = ctx
            .poll_transmit()
            .expect("Startup confirmation should be queued");
        assert!(startup_response.is_startup());

        // Test data transfer in DATA_READY state
        let msg = FmtpMessage::Operational(b"test".to_vec());
        let now_plus_two = now + Duration::from_secs(2); // Advance time
        conn.handle_with_context(
            &Event::LData {
                now: now_plus_two,
                msg: msg.clone(),
            },
            &mut ctx,
        );
        assert_eq!(
            *conn.state(),
            State::DataReady {
                tr: Tr(now_plus_one + Duration::from_secs(40)),
                ts: Ts(now_plus_two + Duration::from_secs(15))
            }
        );
        let data = ctx.poll_transmit().expect("Data packet should be queued");
        assert_eq!(data.header.typ, FmtpType::Operational);
        assert_eq!(
            data.try_to_msg().unwrap(),
            FmtpMessage::Operational(b"test".to_vec())
        );

        // Test heartbeat on Ts timeout
        let heartbeat_time = now_plus_two + Duration::from_secs(16); // Advance past Ts timeout
        conn.handle_with_context(
            &Event::TsTimeout {
                now: heartbeat_time,
            },
            &mut ctx,
        );
        assert_eq!(
            *conn.state(),
            State::DataReady {
                tr: Tr(now_plus_one + Duration::from_secs(40)),
                ts: Ts(heartbeat_time + Duration::from_secs(15))
            }
        );
        let heartbeat = ctx.poll_transmit().expect("Heartbeat should be queued");
        assert!(heartbeat.is_heartbeat());
    }

    #[test]
    fn test_connection_shutdown() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };

        let client_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            remote_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Client,
            initial_target: Target::DataReady,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), client_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config.clone(), &mut ctx, Some("test".to_string()));
        let now = Instant::now();

        // Fast forward to DATA_READY state
        conn.handle_with_context(
            &Event::LSetup {
                id: "test".to_string(),
            },
            &mut ctx,
        );
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        conn.handle_with_context(
            &Event::RIdValid {
                now,
                id: "test".to_string(),
            },
            &mut ctx,
        );
        ctx.poll_transmit().expect("No ID packet to consume");
        ctx.poll_transmit().expect("No accept packet to consume");

        conn.handle_with_context(&Event::LStartup { now }, &mut ctx);
        conn.handle_with_context(&Event::RStartup { now }, &mut ctx);
        ctx.poll_transmit().expect("No startup packet to consume");
        ctx.poll_transmit()
            .expect("No startup confirmation packet to consume");

        // Test shutdown transitions back to READY
        let now = now + Duration::from_secs(1);
        conn.handle_with_context(&Event::LShutdown { now }, &mut ctx);
        assert_eq!(
            *conn.state(),
            State::Ready {
                tr: Tr(now + Duration::from_secs(40))
            }
        );
        let shutdown = ctx
            .poll_transmit()
            .expect("Shutdown packet should be queued");
        assert_eq!(shutdown, FmtpPacket::shutdown());

        // Associate again to DATA_READY
        conn.handle_with_context(&Event::LStartup { now }, &mut ctx);
        conn.handle_with_context(&Event::RStartup { now }, &mut ctx);
        ctx.poll_transmit().expect("No startup packet to consume");
        ctx.poll_transmit()
            .expect("No startup confirmation packet to consume");

        // Test disconnect transitions to IDLE
        conn.handle_with_context(&Event::LDisconnect, &mut ctx);
        assert_eq!(*conn.state(), State::Idle {});
        let shutdown = ctx
            .poll_transmit()
            .expect("Shutdown packet should be queued");
        assert_eq!(shutdown, FmtpPacket::shutdown());
    }

    #[test]
    fn test_error_handling() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };

        let client_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            remote_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Client,
            initial_target: Target::DataReady,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), client_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config, &mut ctx, None);

        // Test invalid ID in setup transitions to ERROR
        conn.handle_with_context(
            &Event::LSetup {
                id: "invalid".to_string(),
            },
            &mut ctx,
        );
        assert!(matches!(*conn.state(), State::Error {}));
        assert!(
            ctx.poll_transmit().is_none(),
            "No packets should be queued in error state"
        );

        // Test error state is terminal (no transitions out)
        conn.handle_with_context(&Event::LDisconnect, &mut ctx);
        assert!(matches!(*conn.state(), State::Error {}));
    }

    #[test]
    fn test_timer_expiry() {
        let mut config = Config {
            bind_address: None,
            connections: HashMap::default(),
            server_ti: None,
        };

        let client_config = ConnectionConfig {
            local_id: FmtpIdentifier::new(b"CLIENT").unwrap(),
            remote_id: FmtpIdentifier::new(b"SERVER").unwrap(),
            ti: Duration::from_secs(30),
            tr: Duration::from_secs(40),
            ts: Duration::from_secs(15),
            remote_addresses: vec![],
            role: Role::Client,
            initial_target: Target::DataReady,
            connect_retry_timer: None,
        };

        config.connections.insert("test".to_string(), client_config);

        let mut ctx = ConnectionContext::default();
        let mut conn = Connection::new(config.clone(), &mut ctx, Some("test".to_string()));
        let now = Instant::now();

        // Get to ID_PENDING state
        conn.handle_with_context(
            &Event::LSetup {
                id: "test".to_string(),
            },
            &mut ctx,
        );
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        assert!(matches!(*conn.state(), State::IdPending { .. }));

        // Test Ti timeout returns to IDLE
        conn.handle_with_context(&Event::TiTimeout, &mut ctx);
        assert!(matches!(*conn.state(), State::Idle {}));

        // Get to DATA_READY state
        conn.handle_with_context(
            &Event::LSetup {
                id: "test".to_string(),
            },
            &mut ctx,
        );
        conn.handle_with_context(&Event::RSetup { now }, &mut ctx);
        conn.handle_with_context(
            &Event::RIdValid {
                now,
                id: "test".to_string(),
            },
            &mut ctx,
        );
        conn.handle_with_context(&Event::LStartup { now }, &mut ctx);
        conn.handle_with_context(&Event::RStartup { now }, &mut ctx);
        assert!(matches!(*conn.state(), State::DataReady { .. }));

        // Test Tr timeout returns to IDLE
        conn.handle_with_context(&Event::TrTimeout { now }, &mut ctx);
        assert!(matches!(*conn.state(), State::Idle {}));
    }
}
