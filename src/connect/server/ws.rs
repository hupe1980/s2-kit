//! The last mile: an authorised WebSocket.
//!
//! Session initiation produces a `websocketToken`; this module is what makes it mean
//! something — the route that checks it, and the transport that carries S2 messages once
//! it has. `S2C §WebSocket based communication` is short and all of it is here: the token
//! in `Authorization: Bearer`, and the S2 session living exactly as long as the socket.
//! `wss://` is the host's [`serve`](super::serve) call rather than this route.
//!
//! ```no_run
//! use std::sync::Arc;
//! use s2_kit::connect::server::{Endpoint, websocket_router};
//! use s2_kit::io::Driver;
//! use s2_kit::prelude::*;
//!
//! # fn build(endpoint: Endpoint) -> axum::Router {
//! websocket_router(endpoint, "/s2", |node, socket| async move {
//!     // `node` is the paired peer the token belonged to: which resource this is.
//!     let mut cem = CemSession::new(CemConfig::default().pre_negotiated(WireProfile::V1_0_0));
//!     cem.open(Timestamp::now());
//!     let mut driver = Driver::new(socket);
//!     let _ = driver.run(&mut cem, |_event, _session| {}).await;
//!     let _ = node;
//! })
//! # }
//! ```

use alloc::string::{String, ToString};
use core::future::Future;

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;

use crate::connect::proto::NodeId;
use crate::io::{Error, TextTransport};

use super::routes::Endpoint;

/// A WebSocket an S2 Connect server accepted, as a transport an [`io::Driver`] can pump.
///
/// [`io::WebSocket`](crate::io::WebSocket) is the client's side of the same thing. They
/// are separate types because the two halves come from different libraries — a client
/// dials with `tokio-tungstenite`, a server upgrades with `axum` — and neither wants to
/// depend on the other's stream type.
///
/// [`io::Driver`]: crate::io::Driver
#[derive(Debug)]
pub struct ServerSocket {
    socket: WebSocket,
}

impl ServerSocket {
    /// Wrap an upgraded socket.
    #[must_use]
    pub fn new(socket: WebSocket) -> Self {
        Self { socket }
    }
}

impl TextTransport for ServerSocket {
    async fn send_text(&mut self, text: String) -> Result<(), Error> {
        self.socket
            .send(WsMessage::Text(text.into()))
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    // The arms are grouped by what happened, not by what is returned: "the stream ended"
    // and "the peer sent a close frame" are different events that mean the same thing.
    #[allow(clippy::match_same_arms)]
    async fn recv_text(&mut self) -> Result<Option<String>, Error> {
        loop {
            match self.socket.recv().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(Error::Transport(e.to_string())),
                Some(Ok(WsMessage::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(WsMessage::Close(_))) => return Ok(None),
                // Pong frames answer our own pings; a ping is answered by axum itself.
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                Some(Ok(WsMessage::Binary(_))) => {
                    return Err(Error::UnexpectedFrame("binary".to_string()));
                }
            }
        }
    }

    async fn ping(&mut self) -> Result<(), Error> {
        self.socket
            .send(WsMessage::Ping(axum::body::Bytes::new()))
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    async fn close(&mut self) {
        let _ = self.socket.send(WsMessage::Close(None)).await;
    }
}

/// Everything the WebSocket route needs: the endpoint, and what to do with a session.
///
/// `Clone` is written out rather than derived: the derive would demand `H: Clone`, and the
/// handler is behind an `Arc` precisely so it does not have to be.
struct WsState<H> {
    endpoint: Endpoint,
    handler: alloc::sync::Arc<H>,
}

impl<H> Clone for WsState<H> {
    fn clone(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            handler: self.handler.clone(),
        }
    }
}

/// A router serving the S2 WebSocket at `path`, for clients that hold a communication
/// token this endpoint issued.
///
/// The token is checked against
/// [`SessionService::authorize_connection`](super::SessionService::authorize_connection)
/// — which consumes it, because `s2-connect-common.yml/CommunicationToken` makes it
/// "valid for a single connection … for maximum 30 seconds". A request with no bearer, an
/// unknown one, an expired one or one already used is `401`, and the upgrade never
/// happens.
///
/// `on_session` is given the paired node the token belonged to and the upgraded socket.
/// The S2 session *is* the WebSocket session, so when it returns the socket is closed.
///
/// Mount it beside [`router`](super::router) or [`lan_router`](super::lan_router) with
/// `Router::merge`, or serve it from another port. Separate for the same reason
/// `lan_router` is: an RM in a LAN never accepts a connection, and should not have to opt
/// *out* of serving one.
pub fn websocket_router<H, Fut>(endpoint: Endpoint, path: &str, on_session: H) -> axum::Router
where
    H: Fn(NodeId, ServerSocket) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    axum::Router::new()
        .route(path, any(upgrade::<H, Fut>))
        .with_state(WsState {
            endpoint,
            handler: alloc::sync::Arc::new(on_session),
        })
}

async fn upgrade<H, Fut>(
    State(state): State<WsState<H>>,
    bearer: super::routes::Bearer,
    upgrade: WebSocketUpgrade,
) -> Response
where
    H: Fn(NodeId, ServerSocket) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let now = state.endpoint.now();
    let Some(node) = state
        .endpoint
        .session()
        .authorize_connection(bearer.as_str(), now)
    else {
        crate::trace::event!(warn, "refused a WebSocket with an unusable token");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    crate::trace::event!(info, node = %node, "accepted an S2 WebSocket");
    let handler = state.handler.clone();
    upgrade.on_upgrade(move |socket| async move {
        handler(node, ServerSocket::new(socket)).await;
    })
}
