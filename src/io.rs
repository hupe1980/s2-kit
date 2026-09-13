//! Driving a session over a real transport.
//!
//! The engines are sans-I/O, so this module is thin on purpose: it flushes what a
//! session wants to send, waits for the next of {a frame, the session's own deadline, a
//! keep-alive ping}, and feeds whatever arrives back in. Everything that decides
//! anything is in [`crate::session`].
//!
//! The transport is a trait, not a WebSocket. WebSocket is what S2 Connect specifies and
//! what [`WebSocket`] implements, but S2 itself is transport-agnostic — Victron's Venus
//! OS carries the same JSON over D-Bus — and keeping the seam here is what stops the
//! session API quietly growing a socket-shaped assumption.
//!
//! ```no_run
//! use s2_kit::prelude::*;
//! use s2_kit::io::{Driver, WebSocket};
//!
//! # async fn run(details: ResourceManagerDetails) -> Result<(), Box<dyn std::error::Error>> {
//! let mut rm = RmSession::new(RmConfig::default(), details);
//! rm.open(Timestamp::now());
//!
//! let socket = WebSocket::connect("wss://cem.local/s2", Some("token")).await?;
//! let mut driver = Driver::new(socket);
//!
//! loop {
//!     driver.step(&mut rm).await?;
//!     while let Some(event) = rm.poll_event() {
//!         match event {
//!             RmEvent::Ready { control_type } => { /* send a description */ }
//!             RmEvent::Instruction(i) => { /* i.explanation says what to do */ }
//!             RmEvent::Closed { .. } => return Ok(()),
//!             _ => {}
//!         }
//!     }
//! }
//! # }
//! ```

use alloc::string::{String, ToString};
use core::future::Future;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::session::Session;
use crate::types::{Duration, Timestamp};

/// S2 Connect's keep-alive interval.
///
/// `S2C §Keepalive & heartbeat`: "S2 WebSockets implementations **should** send a ping
/// frame every 30 seconds, and **must not** wait more than 60 seconds between sending
/// ping frames."
pub const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Something went wrong at the transport.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The transport failed.
    #[error("transport error: {0}")]
    Transport(String),
    /// The peer closed the connection.
    #[error("the peer closed the connection")]
    Closed,
    /// A frame arrived that is not S2: a binary frame, for instance.
    #[error("unexpected frame: {0}")]
    UnexpectedFrame(String),
}

/// A two-way channel that carries S2 messages as text.
pub trait TextTransport {
    /// Send one message.
    fn send_text(&mut self, text: String) -> impl Future<Output = Result<(), Error>> + Send;
    /// Wait for one message. `None` when the peer closed cleanly.
    fn recv_text(&mut self) -> impl Future<Output = Result<Option<String>, Error>> + Send;
    /// Send a keep-alive, if the transport has one.
    fn ping(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
    /// Close.
    fn close(&mut self) -> impl Future<Output = ()> + Send;
}

/// A WebSocket, as S2 Connect specifies it.
pub struct WebSocket<S> {
    stream: tokio_tungstenite::WebSocketStream<S>,
}

impl WebSocket<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    /// Connect to a URL, with the bearer token S2 Connect's session initiation issued.
    ///
    /// `S2C §Authentication`: "the client **must** authenticate itself using the
    /// commToken in the authorization header of the websocket connection request".
    ///
    /// TLS is verified against the **public** PKI, which is right for a WAN endpoint and
    /// wrong for a LAN one: a `wss://EVSE1038.local/s2` certificate is self-signed, and
    /// what makes it trustworthy is the CA pinned during pairing. Use
    /// [`connect_with_policy`](Self::connect_with_policy) there.
    pub async fn connect(url: &str, bearer: Option<&str>) -> Result<Self, Error> {
        Self::dial(url, bearer, None).await
    }

    /// Connect under a [`TlsPolicy`](crate::connect::tls::TlsPolicy) — which, after a LAN
    /// pairing, is the CA that pairing pinned.
    ///
    /// This is the companion of [`connect::client::Session`](crate::connect::client) and
    /// the one to use in a LAN. The session initiation that produced the communication
    /// token ran against the pinned CA; connecting the WebSocket against a bundled root
    /// list instead would either fail outright — which is what it does — or, worse,
    /// succeed against a different server.
    ///
    /// ```no_run
    /// # use s2_kit::io::WebSocket;
    /// # async fn run(paired: s2_kit::connect::client::Paired, url: &str, token: &str)
    /// # -> Result<(), Box<dyn std::error::Error>> {
    /// let socket = WebSocket::connect_with_policy(url, Some(token), &paired.policy()).await?;
    /// # let _ = socket;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "connect-client")]
    #[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
    pub async fn connect_with_policy(
        url: &str,
        bearer: Option<&str>,
        policy: &crate::connect::tls::TlsPolicy,
    ) -> Result<Self, Error> {
        // A session must never run unauthenticated: the pairing-only policy exists for
        // the four pairing requests and nothing else.
        if !policy.authenticates() {
            return Err(Error::Transport(
                "a session must not run under the pairing-only TLS policy".to_string(),
            ));
        }
        let client = crate::connect::tls::TlsClient::new(policy)
            .map_err(|e| Error::Transport(e.to_string()))?;
        let connector = tokio_tungstenite::Connector::Rustls(client.config());
        Self::dial(url, bearer, Some(connector)).await
    }

    async fn dial(
        url: &str,
        bearer: Option<&str>,
        connector: Option<tokio_tungstenite::Connector>,
    ) -> Result<Self, Error> {
        let mut request = url
            .into_client_request()
            .map_err(|e| Error::Transport(e.to_string()))?;
        if let Some(token) = bearer {
            let value = alloc::format!("Bearer {token}")
                .parse()
                .map_err(|_| Error::Transport("invalid bearer token".to_string()))?;
            request.headers_mut().insert("Authorization", value);
        }
        let (stream, _) =
            tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector)
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self { stream })
    }
}

impl<S> WebSocket<S> {
    /// Wrap an already-established WebSocket — one a server accepted, for instance.
    pub fn from_stream(stream: tokio_tungstenite::WebSocketStream<S>) -> Self {
        Self { stream }
    }
}

impl<S> TextTransport for WebSocket<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    async fn send_text(&mut self, text: String) -> Result<(), Error> {
        self.stream
            .send(WsMessage::text(text))
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    #[allow(clippy::match_same_arms)] // grouped by reason; see the comments
    async fn recv_text(&mut self) -> Result<Option<String>, Error> {
        loop {
            match self.stream.next().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(Error::Transport(e.to_string())),
                Some(Ok(WsMessage::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(WsMessage::Close(_))) => return Ok(None),
                // Ping and pong are handled by the library; a binary frame is not S2.
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Ok(WsMessage::Binary(_))) => {
                    return Err(Error::UnexpectedFrame("binary".to_string()));
                }
            }
        }
    }

    async fn ping(&mut self) -> Result<(), Error> {
        self.stream
            .send(WsMessage::Ping(tokio_tungstenite::tungstenite::Bytes::new()))
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    async fn close(&mut self) {
        let _ = self.stream.close(None).await;
    }
}

/// Pumps a session over a transport.
///
/// One `step` flushes everything the session wants to send, then waits for whichever
/// comes first: an inbound frame, the session's next deadline, or the keep-alive
/// interval. The application keeps the loop, and with it the freedom to do anything else
/// between steps.
pub struct Driver<T> {
    transport: T,
    ping_interval: Duration,
    /// The clock. Replaceable so that a test can drive a driver too.
    now: fn() -> Timestamp,
    /// When a keep-alive was last sent, or `None` before the first step.
    ///
    /// The interval is measured from here rather than from the start of each step. A
    /// driver that restarts the timer every time it goes round the loop never pings a
    /// connection that is busy — and `S2C §Keepalive & heartbeat` does not say "ping an
    /// idle connection", it says an implementation "**must not** wait more than 60
    /// seconds between sending ping frames".
    last_ping: Option<Timestamp>,
}

impl<T: TextTransport> Driver<T> {
    /// A driver over this transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            ping_interval: PING_INTERVAL,
            now: Timestamp::now,
            last_ping: None,
        }
    }

    /// Use a different clock — a virtual one, in a test.
    #[must_use]
    pub fn with_clock(mut self, now: fn() -> Timestamp) -> Self {
        self.now = now;
        self
    }

    /// Send pings at a different interval. S2 Connect allows up to sixty seconds.
    #[must_use]
    pub fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// The transport, for a caller that wants to close it itself.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Run one step of a session, whichever role it is.
    ///
    /// Flush, wait for the first of {a frame, the session's deadline, the keep-alive
    /// interval}, feed the result back in, flush again. Everything that decides anything
    /// is in the session; this function decides only what to wait on.
    pub async fn step<S: Session>(&mut self, session: &mut S) -> Result<(), Error> {
        self.flush(session).await?;
        if session.state().is_closed() {
            self.transport.close().await;
            return Ok(());
        }
        let deadline = session.poll_timeout();
        match self.wait(deadline).await? {
            Wake::Text(text) => {
                let inbound = session.handle_text(&text, (self.now)());
                crate::trace::event!(
                    debug,
                    kind = ?inbound.kind,
                    status = ?inbound.status,
                    violations = inbound.report.violations().len(),
                    "received"
                );
                // A refusal is the thing an operator actually needs to see, and the rule
                // identifier is what makes it greppable across a fleet.
                if inbound.report.has_errors() {
                    crate::trace::event!(
                        warn,
                        kind = ?inbound.kind,
                        diagnostic = ?inbound.report.diagnostic_label(),
                        "refused an inbound message"
                    );
                }
            }
            Wake::Timeout => {
                crate::trace::event!(trace, "deadline reached");
                session.handle_timeout((self.now)());
            }
            Wake::Ping => {
                self.last_ping = Some((self.now)());
                self.transport.ping().await?;
            }
            Wake::Closed => {
                crate::trace::event!(info, "the transport closed");
                session.transport_closed((self.now)());
            }
        }
        self.flush(session).await
    }

    /// Send everything the session has queued.
    async fn flush<S: Session>(&mut self, session: &mut S) -> Result<(), Error> {
        while let Some(out) = session.poll_transmit() {
            crate::trace::event!(
                debug,
                kind = %out.kind.as_str(),
                message_id = ?out.message_id,
                bytes = out.text.len(),
                "sending"
            );
            self.transport.send_text(out.text).await?;
        }
        Ok(())
    }

    /// Run a session to completion: steps until it closes.
    ///
    /// The application still sees every event, through the callback, but no longer owns
    /// the loop. For anything that wants to do other work between steps, use
    /// [`Self::step`].
    pub async fn run<S, F>(&mut self, session: &mut S, mut on_event: F) -> Result<(), Error>
    where
        S: Session,
        F: FnMut(S::Event, &mut S),
    {
        while !session.state().is_closed() {
            self.step(session).await?;
            while let Some(event) = session.poll_event() {
                on_event(event, session);
            }
        }
        self.flush(session).await?;
        self.transport.close().await;
        Ok(())
    }

    async fn wait(&mut self, deadline: Option<Timestamp>) -> Result<Wake, Error> {
        let now = (self.now)();
        // A session with no deadline still has to be woken for the keep-alive, so
        // "never" is expressed as a span no ping interval will ever exceed rather than
        // as an infinite sleep.
        let until_deadline = deadline.map_or(Duration::MAX, |at| at.saturating_duration_since(now));
        let until_ping = match self.last_ping {
            // The first step starts the clock rather than pinging immediately: a socket
            // that has just been opened does not need proof that it is alive.
            None => {
                self.last_ping = Some(now);
                self.ping_interval
            }
            Some(last) => self
                .ping_interval
                .checked_sub(now.saturating_duration_since(last))
                .unwrap_or(Duration::ZERO),
        };

        let (sleep_for, wake) = if until_deadline <= until_ping {
            (until_deadline, Wake::Timeout)
        } else {
            (until_ping, Wake::Ping)
        };

        tokio::select! {
            frame = self.transport.recv_text() => match frame? {
                Some(text) => Ok(Wake::Text(text)),
                None => Ok(Wake::Closed),
            },
            () = tokio::time::sleep(core::time::Duration::from(sleep_for)) => Ok(wake),
        }
    }
}

enum Wake {
    Text(String),
    Timeout,
    Ping,
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{RmConfig, RmSession};
    use alloc::collections::VecDeque;
    use alloc::vec::Vec;

    /// A transport that hands back a script and records what was written.
    struct Scripted {
        inbound: VecDeque<String>,
        outbound: Vec<String>,
        pings: usize,
    }

    impl TextTransport for Scripted {
        async fn send_text(&mut self, text: String) -> Result<(), Error> {
            // A yield point, so the mock behaves like a transport that can be
            // interleaved rather than one that always completes instantly.
            tokio::task::yield_now().await;
            self.outbound.push(text);
            Ok(())
        }
        async fn recv_text(&mut self) -> Result<Option<String>, Error> {
            if let Some(text) = self.inbound.pop_front() {
                return Ok(Some(text));
            }
            // Nothing more scripted: block for ever, so the timer wins the select.
            core::future::pending::<()>().await;
            unreachable!()
        }
        async fn ping(&mut self) -> Result<(), Error> {
            tokio::task::yield_now().await;
            self.pings += 1;
            Ok(())
        }
        async fn close(&mut self) {
            tokio::task::yield_now().await;
        }
    }

    fn clock() -> Timestamp {
        "2024-01-01T12:00:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH)
    }

    #[tokio::test]
    async fn a_step_flushes_the_outbox_and_feeds_back_what_arrives() {
        let details = crate::testing::battery_details();
        let mut rm = RmSession::new(RmConfig::default(), details);
        rm.open(clock());

        let transport = Scripted {
            inbound: VecDeque::from([
                r#"{"message_type":"Handshake","message_id":"c1","role":"CEM","supported_protocol_versions":["1.0.0"]}"#.to_string(),
            ]),
            outbound: Vec::new(),
            pings: 0,
        };
        let mut driver = Driver::new(transport).with_clock(clock);

        driver.step(&mut rm).await.unwrap();

        let written = &driver.transport_mut().outbound;
        assert!(written[0].contains(r#""message_type":"Handshake""#));
        // And the CEM's handshake was answered.
        assert!(written.iter().any(|m| m.contains("ReceptionStatus")));
    }

    /// A transport that answers every `recv_text` after `gap`, for ever.
    ///
    /// A connection that is *busy*, as opposed to the silent one the other test uses.
    struct Chatty {
        gap: core::time::Duration,
        pings: usize,
    }

    impl TextTransport for Chatty {
        // The trait is async; a mock that drops what it is given is not.
        #[allow(clippy::unused_async_trait_impl)]
        async fn send_text(&mut self, _text: String) -> Result<(), Error> {
            Ok(())
        }
        async fn recv_text(&mut self) -> Result<Option<String>, Error> {
            tokio::time::sleep(self.gap).await;
            Ok(Some(
                r#"{"message_type":"PowerMeasurement","message_id":"c1",
                    "measurement_timestamp":"2024-01-01T12:00:00Z",
                    "values":[{"commodity_quantity":"ELECTRIC.POWER.L1","value":1.0}]}"#
                    .to_string(),
            ))
        }
        #[allow(clippy::unused_async_trait_impl)]
        async fn ping(&mut self) -> Result<(), Error> {
            self.pings += 1;
            Ok(())
        }
        #[allow(clippy::unused_async_trait_impl)]
        async fn close(&mut self) {}
    }

    /// A clock reading tokio's virtual time, so the driver and the runtime agree.
    fn virtual_clock() -> Timestamp {
        use std::sync::OnceLock;
        static START: OnceLock<tokio::time::Instant> = OnceLock::new();
        let start = *START.get_or_init(tokio::time::Instant::now);
        let elapsed = tokio::time::Instant::now().saturating_duration_since(start);
        Timestamp::from_unix(1_700_000_000, 0)
            .checked_add(Duration::from_millis(
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            ))
            .unwrap_or(Timestamp::UNIX_EPOCH)
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_connection_still_gets_a_keep_alive() {
        // The bug this pins: an interval measured from the start of each step never
        // elapses on a connection that has traffic, so a driver doing its job stops
        // pinging — and `S2C §Keepalive & heartbeat` says an implementation "must not
        // wait more than 60 seconds between sending ping frames", not "unless it is
        // busy". A frame every hundred milliseconds against a one-second interval is a
        // connection that is never idle for a whole interval and must be pinged anyway.
        let details = crate::testing::battery_details();
        let mut rm = RmSession::new(RmConfig::default(), details);
        rm.open(virtual_clock());

        let transport = Chatty {
            gap: core::time::Duration::from_millis(100),
            pings: 0,
        };
        let mut driver = Driver::new(transport)
            .with_clock(virtual_clock)
            .with_ping_interval(Duration::from_secs(1));

        // Thirty-odd steps is a little over three seconds of traffic.
        for _ in 0..34 {
            driver.step(&mut rm).await.unwrap();
        }
        assert!(
            driver.transport_mut().pings >= 2,
            "expected the keep-alive to fire about once a second, got {}",
            driver.transport_mut().pings
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_connection_gets_a_keep_alive() {
        let details = crate::testing::battery_details();
        let mut rm = RmSession::new(RmConfig::default(), details);
        rm.open(clock());
        let transport = Scripted {
            inbound: VecDeque::new(),
            outbound: Vec::new(),
            pings: 0,
        };
        let mut driver = Driver::new(transport)
            .with_clock(clock)
            .with_ping_interval(Duration::from_secs(1));
        // The handshake is waiting for an acknowledgement with a five-second deadline,
        // so the one-second ping interval wins.
        driver.step(&mut rm).await.unwrap();
        assert_eq!(driver.transport_mut().pings, 1);
    }
}
