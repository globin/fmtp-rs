//! FMTP HTTP server crate.
//!
//! This crate provides an HTTP API to send data and control commands to FMTP connections.
//!
//! # HTTP API Routes
//!
//! ## List All Connections
//! `GET /`
//!
//! Returns a debug representation of all active FMTP connections.
//!
//! ## Get Connection Details
//! `GET /{conn}`
//!
//! Returns details about a specific connection identified by `conn`.
//!
//! ## Send Data
//! `POST /{conn}`
//!
//! Sends operational data over the specified FMTP connection. The request body
//! contains the raw data to be sent.
//!
//! ## Start Association
//! `POST /{conn}/associate`
//!
//! Initiates an FMTP association on the specified connection (MT-ASSOC service primitive).
//!
//! ## Stop Association
//! `POST /{conn}/shutdown`
//!
//! Stops an FMTP association without closing the underlying connection (MT-STOP service primitive).
//!
//! ## Disconnect
//! `POST /{conn}/disconnect`
//!
//! Stops the FMTP association and closes the underlying connection (MT-DIS service primitive).
//!
//! # Error Responses
//!
//! - `404 Not Found`: Returned when the specified connection does not exist
//! - `500 Internal Server Error`: Returned when a command cannot be sent to the connection

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use fmtp_core::{Config, ConnectionConfig, FmtpIdentifier, FmtpMessage, Role, Target, UserCommand};
use fmtp_tokio::{ConnectionState, Server};
use tokio::{net::TcpListener, spawn, sync::Mutex};
use tracing::{error, info, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Clone)]
struct AppState {
    connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
}

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

    let fmtp_server = Server::new(Config {
        bind_address: Some("127.0.0.1:8500".parse()?),
        connections: [(
            "fmtp".to_string(),
            ConnectionConfig {
                local_id: FmtpIdentifier::new(b"SERVER")?,
                remote_id: FmtpIdentifier::new(b"CLIENT")?,
                ti: Duration::from_secs(30),
                tr: Duration::from_secs(40),
                ts: Duration::from_secs(15),
                remote_addresses: vec!["127.0.0.1:8500".parse()?],
                role: Role::Server,
                initial_target: Target::DataReady,
                connect_retry_timer: None,
            },
        )]
        .into(),
        server_ti: Some(Duration::from_secs(30)),
    });
    let connections = fmtp_server.connections();

    spawn(async move {
        match fmtp_server.run().await {
            Ok(handles) => handles.join_all().await.iter().for_each(|res| {
                if let Err(e) = res {
                    error!("FMTP error: {e}");
                } else {
                    info!("handle finished");
                }
            }),
            Err(e) => error!("FMTP server error: {e}"),
        }
        info!("FMTP finished");
    });

    let state = AppState { connections };
    // build our application with a single route
    let app = Router::new()
        .route("/", get(get_all))
        .route("/{conn}", get(conn_get))
        .route("/{conn}", post(conn_data))
        .route("/{conn}/associate", post(conn_associate))
        .route("/{conn}/shutdown", post(conn_shutdown))
        .route("/{conn}/disconnect", post(conn_disconnect))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:8000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

async fn get_all(State(state): State<AppState>) -> String {
    format!("{:#?}", *(state.connections.lock().await))
}

async fn conn_get(
    Path(conn_id): Path<String>,
    State(state): State<AppState>,
) -> Result<String, StatusCode> {
    if let Some(conn) = state.connections.lock().await.get(&conn_id) {
        Ok(format!("{conn:#?}"))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn conn_data(
    Path(conn_id): Path<String>,
    State(state): State<AppState>,
    data: Bytes,
) -> Result<(), (StatusCode, String)> {
    if let Some(conn) = state.connections.lock().await.get(&conn_id) {
        conn.command_tx
            .send(UserCommand::Data {
                msg: FmtpMessage::Operational(data.to_vec()),
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, String::new()))
    }
}

async fn conn_associate(
    Path(conn_id): Path<String>,
    State(state): State<AppState>,
) -> Result<(), (StatusCode, String)> {
    if let Some(conn) = state.connections.lock().await.get(&conn_id) {
        conn.command_tx
            .send(UserCommand::Startup)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, String::new()))
    }
}

async fn conn_shutdown(
    Path(conn_id): Path<String>,
    State(state): State<AppState>,
) -> Result<(), (StatusCode, String)> {
    if let Some(conn) = state.connections.lock().await.get(&conn_id) {
        conn.command_tx
            .send(UserCommand::Shutdown)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, String::new()))
    }
}

async fn conn_disconnect(
    Path(conn_id): Path<String>,
    State(state): State<AppState>,
) -> Result<(), (StatusCode, String)> {
    if let Some(conn) = state.connections.lock().await.get(&conn_id) {
        conn.command_tx
            .send(UserCommand::Disconnect)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, String::new()))
    }
}
