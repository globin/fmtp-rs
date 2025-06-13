//! Core implementation of the FMTP (Flight Message Transfer Protocol) as specified in
//! EUROCONTROL FMTP v2.0 specification.
//!
//! This crate provides the core state machine and types for implementing FMTP connections.
//! It handles:
//! - Connection establishment and identification
//! - Data transfer and heartbeat mechanisms
//! - Connection state management
//! - Timer management
//!
//! # Architecture
//!
//! The core components are:
//! - [`Connection`]: The main state machine implementing the FMTP protocol
//! - [`ConnectionContext`]: Manages packet and message queues
//! - [`Event`] and [`UserCommand`]: Drive state machine transitions
//! - [`FmtpPacket`] and [`FmtpMessage`]: Protocol data units
//!
//! # Example
//!
//! A short demonstration of setting up a connection and handling events, for advanced usage check out the [`fmtp-tokio`] crate which
//! provides an async interface for FMTP connections.
//!
//! ```
//! use std::time::Duration;
//! use fmtp_core::{Connection, ConnectionContext, Config, UserCommand, Event, Target, FmtpIdentifier, Role, ConnectionConfig};
//!
//! let mut ctx = ConnectionContext::default();
//! let config = Config {
//!     connections: [(
//!         "local".to_string(),
//!         ConnectionConfig {
//!             remote_addresses: vec![
//!                 "127.0.0.1:8500".parse().unwrap(),
//!                 "[::1]:8500".parse().unwrap(),
//!             ],
//!             remote_id: FmtpIdentifier::new("SERVER".as_bytes()).unwrap(),
//!             local_id: FmtpIdentifier::new("CLIENT".as_bytes()).unwrap(),
//!             ti: Duration::from_secs(30),
//!             ts: Duration::from_secs(15),
//!             tr: Duration::from_secs(40),
//!             role: Role::Client,
//!             initial_target: Target::DataReady,
//!             connect_retry_timer: Some(Duration::from_secs(3)),
//!         },
//!     )]
//!     .into(),
//!     bind_address: None,
//!     server_ti: None,
//! };
//! let mut conn = Connection::new(config, &mut ctx, Some("local".to_string()));
//!
//! // Handle events
//! conn.handle_with_context(&Event::LSetup {
//!     id: "test".to_string()
//! }, &mut ctx);
//! ```

mod config;
mod connection;
mod event;
mod identifier;
mod message;
mod packet;

pub use config::{Config, ConnectionConfig, Role, Target};
pub use connection::{Connection, ConnectionContext, State};
pub use event::{Event, UserCommand};
pub use identifier::FmtpIdentifier;
pub use message::FmtpMessage;
pub use packet::{FmtpPacket, FmtpType};

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

/// Timer for the identification phase of connection establishment.
///
/// This timer runs when waiting for an identification message from the remote endpoint.
/// If it expires before valid identification is received, the connection attempt fails.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ti(pub Instant);

/// Timer for expecting responses during data transfer.
///
/// This timer runs when waiting for any response (heartbeat or data) from the remote endpoint.
/// If it expires without receiving a response, the connection is considered lost.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Tr(pub Instant);

/// Timer for sending periodic heartbeat messages.
///
/// This timer ensures regular heartbeats are sent to maintain the connection when
/// there is no data to transfer. It is reset whenever data is sent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ts(pub Instant);

/// A future that completes when there is a packet ready to transmit.
///
/// Created by [`ConnectionContext::transmit_future`], this future is used in async
/// contexts to wait for the next packet that needs to be sent over the connection.
pub struct TxFut(Option<FmtpPacket>);
impl Future for TxFut {
    type Output = FmtpPacket;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(packet) = self.0.take() {
            Poll::Ready(packet)
        } else {
            Poll::Pending
        }
    }
}

/// A future that completes when there is a message ready to receive.
///
/// Created by [`ConnectionContext::receive_future`], this future is used in async
/// contexts to wait for the next received message to be processed.
pub struct RxFut(Option<FmtpMessage>);
impl Future for RxFut {
    type Output = FmtpMessage;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(packet) = self.0.take() {
            Poll::Ready(packet)
        } else {
            Poll::Pending
        }
    }
}
