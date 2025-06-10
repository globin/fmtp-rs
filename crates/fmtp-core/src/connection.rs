use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    time::Instant,
};

use crate::{
    Config, Event, FmtpMessage, FmtpPacket, RxFut, Target, Ti, Tr, Ts, TxFut, event::UserCommand,
};

use statig::{
    Response, StateOrSuperstate,
    prelude::{InitializedStateMachine, IntoStateMachineExt as _},
    state_machine,
};
use tracing::{debug, error, trace, warn};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConnectionContext {
    send_queue: VecDeque<FmtpPacket>,
    recv_queue: VecDeque<FmtpMessage>,
}
impl ConnectionContext {
    pub fn poll_transmit(&mut self) -> Option<FmtpPacket> {
        self.send_queue.pop_front()
    }
    pub fn transmit_future(&mut self) -> TxFut {
        TxFut(self.poll_transmit())
    }
    pub fn poll_receive(&mut self) -> Option<FmtpMessage> {
        self.recv_queue.pop_front()
    }
    pub fn receive_future(&mut self) -> RxFut {
        RxFut(self.poll_receive())
    }
}

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
            Event::UserCommand(UserCommand::Shutdown { .. }) => self.target = Target::Ready,
            Event::UserCommand(UserCommand::Startup { .. }) => self.target = Target::DataReady,
            Event::UserCommand(UserCommand::Disconnect) => self.target = Target::Idle,
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
            Event::RDisconnect | Event::UserCommand(UserCommand::Disconnect) => Response::Handled,
            Event::UserCommand(UserCommand::Setup { id }) => {
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
            | Event::UserCommand(
                UserCommand::Data { .. }
                | UserCommand::Shutdown { .. }
                | UserCommand::Startup { .. },
            ) => {
                error!("Unexpected Event {event:?} in IDLE");
                Response::Transition(State::error())
            }
        }
    }

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
            Event::RDisconnect | Event::UserCommand(UserCommand::Disconnect) => {
                Response::Transition(State::idle())
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
            | Event::UserCommand(
                UserCommand::Setup { .. }
                | UserCommand::Data { .. }
                | UserCommand::Shutdown { .. }
                | UserCommand::Startup { .. },
            ) => {
                error!("Unexpected Event {event:?} in CONNECTION_PENDING");
                Response::Transition(State::error())
            }
        }
    }

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
            | Event::UserCommand(UserCommand::Disconnect) => Response::Transition(State::idle()),
            Event::RSetup { .. }
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::UserCommand(
                UserCommand::Setup { .. }
                | UserCommand::Shutdown { .. }
                | UserCommand::Startup { .. }
                | UserCommand::Data { .. },
            ) => {
                error!("Unexpected Event {event:?} in ID_PENDING");
                Response::Transition(State::error())
            }
        }
    }

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
            | Event::UserCommand(UserCommand::Disconnect) => Response::Transition(State::idle()),
            Event::RSetup { .. }
            | Event::TrTimeout { .. }
            | Event::TsTimeout { .. }
            | Event::UserCommand(
                UserCommand::Setup { .. }
                | UserCommand::Shutdown { .. }
                | UserCommand::Startup { .. }
                | UserCommand::Data { .. },
            ) => {
                error!("Unexpected Event {event:?} in ID_PENDING");
                Response::Transition(State::error())
            }
        }
    }

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
            Event::UserCommand(UserCommand::Startup { now }) => {
                context.send_queue.push_back(FmtpPacket::startup());
                Response::Transition(State::association_pending(Tr(*now + config.tr)))
            }
            Event::RDisconnect | Event::UserCommand(UserCommand::Disconnect) => {
                Response::Transition(State::idle())
            }
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
            | Event::UserCommand(
                UserCommand::Setup { .. } | UserCommand::Data { .. } | UserCommand::Shutdown { .. },
            ) => {
                error!("Unexpected Event {event:?} in READY");
                Response::Transition(State::error())
            }
        }
    }

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
            Event::UserCommand(UserCommand::Disconnect) => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::idle())
            }
            Event::UserCommand(UserCommand::Shutdown { now }) => {
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
            | Event::UserCommand(
                UserCommand::Setup { .. } | UserCommand::Data { .. } | UserCommand::Startup { .. },
            ) => {
                error!("Unexpected Event {event:?} in ASSOCIATION_PENDING");
                Response::Transition(State::error())
            }
        }
    }

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
            Event::TrTimeout { .. } | Event::RDisconnect => Response::Transition(State::idle()),
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
            Event::UserCommand(UserCommand::Disconnect) => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::idle())
            }
            Event::UserCommand(UserCommand::Data { now, msg }) => {
                context
                    .send_queue
                    .push_back(FmtpPacket::from_msg(msg.clone()));
                Response::Transition(State::data_ready(*tr, Ts(*now + config.ts)))
            }
            Event::UserCommand(UserCommand::Shutdown { now }) => {
                context.send_queue.push_back(FmtpPacket::shutdown());
                Response::Transition(State::ready(Tr(*now + config.tr)))
            }
            Event::RSetup { .. }
            | Event::RReject
            | Event::RAccept { .. }
            | Event::RIdInvalid
            | Event::RIdValid { .. }
            | Event::TiTimeout
            | Event::UserCommand(UserCommand::Setup { .. } | UserCommand::Startup { .. }) => {
                error!("Unexpected Event {event:?} in DATA_READY");
                Response::Transition(State::error())
            }
        }
    }

    #[state]
    fn error() -> Response<State> {
        Response::Handled
    }
}
