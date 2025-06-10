# FMTP implementation in Rust

This implements the flight message transfer protocol (FMTP) in version 2.0 as
specified by EUROCONTROL.

## Crates

### fmtp-core

Main state machine implementing the core protocol logic.

### fmtp-tokio

Tokio async implementation of FMTP connections and a server able to handle
multiple client and server connections.

### fmtp-http

Uses an axum HTTP server to control the fmtp-tokio server and unidirectionally
enables sending data through FMTP to the remote partner.
