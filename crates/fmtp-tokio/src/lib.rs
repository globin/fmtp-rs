//! Tokio-based implementation of the FMTP protocol
//!
//! This crate provides an asynchronous implementation of the Flight Message Transfer Protocol (FMTP)
//! using the Tokio runtime. It implements both client and server roles of the protocol.
//!
//! # Features
//!
//! - Asynchronous I/O with Tokio
//! - Support for both client and server roles
//! - Connection management and state tracking
//! - Event-driven architecture with message passing
//!
//! # Examples
//!
//! See the `examples` directory for complete examples of both a single client and a multi-connection daemon implementation.
//!
//! The `fmtp-http` crate provides a unidirectional HTTP interface to send messages to FMTP connections and is a further example for use of this crate.

mod connection;
mod daemon;

pub use connection::{Connection, ConnectionEvent};
pub use daemon::{ConnectionState, Daemon};
