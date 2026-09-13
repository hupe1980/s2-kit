//! A bare-WebSocket Customer Energy Manager, for talking to an implementation this crate
//! did not write.
//!
//! The official S2 example Resource Managers (`flexiblepower/s2-example-implementations`)
//! are WebSocket **clients**: they read `CEM_URL` and dial it, speaking plain S2 JSON with
//! no S2 Connect at all. So the thing they need on the other end is exactly this — a
//! WebSocket server that hands each connection to a [`CemSession`] and drives it with
//! [`Driver`].
//!
//! ```console
//! $ cargo run --features connect-server,tokio --example interop_cem -- 127.0.0.1:1234
//! $ CEM_URL=ws://127.0.0.1:1234 CONTROL_TYPE=FRBC ./battery
//! ```
//!
//! It prints one line per event and exits non-zero if the conversation produced a
//! refusal, which is what makes it usable as an interop *check* rather than a demo.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::any;
use s2_kit::io::{Driver, Error as IoError, TextTransport};
use s2_kit::prelude::*;
use s2_kit::session::{CemEvent, SessionState};

/// The upgraded socket, as a transport the driver can pump.
struct Socket(WebSocket);

impl TextTransport for Socket {
    async fn send_text(&mut self, text: String) -> Result<(), IoError> {
        self.0
            .send(WsMessage::Text(text.into()))
            .await
            .map_err(|e| IoError::Transport(e.to_string()))
    }

    async fn recv_text(&mut self) -> Result<Option<String>, IoError> {
        loop {
            match self.0.recv().await {
                None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
                Some(Err(e)) => return Err(IoError::Transport(e.to_string())),
                Some(Ok(WsMessage::Text(t))) => return Ok(Some(t.to_string())),
                Some(Ok(_)) => {}
            }
        }
    }

    async fn ping(&mut self) -> Result<(), IoError> {
        self.0
            .send(WsMessage::Ping(axum::body::Bytes::new()))
            .await
            .map_err(|e| IoError::Transport(e.to_string()))
    }

    async fn close(&mut self) {
        let _ = self.0.send(WsMessage::Close(None)).await;
    }
}

static REFUSALS: AtomicUsize = AtomicUsize::new(0);
static INSTRUCTED: AtomicUsize = AtomicUsize::new(0);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:1234".to_string());
    let seconds: u64 = std::env::var("INTEROP_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let app = axum::Router::new()
        .route("/", any(upgrade))
        .route("/{*rest}", any(upgrade));
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("listening on ws://{addr}");

    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    server.abort();

    let refused = REFUSALS.load(Ordering::Relaxed);
    let instructed = INSTRUCTED.load(Ordering::Relaxed);
    println!("refusals: {refused}, instructions accepted by the peer: {instructed}");
    if refused > 0 {
        return Err("the conversation produced refusals".into());
    }
    Ok(())
}

async fn upgrade(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|socket| async move {
        if let Err(e) = run(Socket(socket)).await {
            eprintln!("session ended: {e}");
        }
    })
}

async fn run(socket: Socket) -> Result<(), Box<dyn std::error::Error>> {
    let mut cem = CemSession::new(CemConfig::default());
    cem.open(Timestamp::now());
    let mut driver = Driver::new(socket);
    let described = Arc::new(AtomicUsize::new(0));

    while !cem.state().is_closed() {
        driver.step(&mut cem).await?;
        while let Some(event) = cem.poll_event() {
            match event {
                CemEvent::Negotiated { version, profile } => {
                    println!("negotiated {version} ({profile})");
                }
                CemEvent::ResourceDescribed(details) => {
                    println!(
                        "resource {} offers {:?}",
                        details.resource_id, details.available_control_types
                    );
                    // Activate the first control type the resource actually offers.
                    if let Some(ct) = details
                        .available_control_types
                        .iter()
                        .find(|c| c.is_controllable())
                    {
                        cem.select_control_type(*ct, Timestamp::now())?;
                    }
                }
                CemEvent::Ready { control_type } => println!("active: {control_type:?}"),
                CemEvent::Description { kind, message } => {
                    println!("description: {kind}");
                    // Once the system description is in, instruct something real.
                    if kind == MessageKind::FrbcSystemDescription
                        && described.fetch_add(1, Ordering::Relaxed) == 0
                        && let Message::FrbcSystemDescription(system) = &message
                        && let Some(actuator) = system.actuators.first()
                        && let Some(mode) = actuator.operation_modes.first()
                    {
                        let now = Timestamp::now();
                        let sent = cem.instruct(
                            s2_kit::types::frbc::Instruction {
                                message_id: Id::generate(),
                                id: Id::generate(),
                                actuator_id: actuator.id,
                                operation_mode: mode.id,
                                operation_mode_factor: 0.5,
                                execution_time: now,
                                abnormal_condition: false,
                            },
                            now,
                        );
                        match sent {
                            Ok(_) => println!("instructed {} at factor 0.5", mode.id),
                            Err(e) => println!("could not instruct: {e}"),
                        }
                    }
                }
                CemEvent::Status { kind, .. } => println!("status: {kind}"),
                CemEvent::Measurement(_) => println!("measurement"),
                CemEvent::Forecast(_) => println!("forecast"),
                CemEvent::InstructionStatus {
                    instruction_id,
                    status,
                    ..
                } => {
                    println!("instruction {instruction_id} is {status:?}");
                    INSTRUCTED.fetch_add(1, Ordering::Relaxed);
                }
                CemEvent::Refused {
                    kind,
                    status,
                    report,
                } => {
                    println!("REFUSED {kind:?} with {status:?}: {report}");
                    REFUSALS.fetch_add(1, Ordering::Relaxed);
                }
                CemEvent::Warnings { kind, report } => {
                    println!("warning on inbound {kind}: {report}");
                }
                CemEvent::OutboundWarnings { kind, report } => {
                    println!("warning on our own {kind}: {report}");
                }
                CemEvent::Nacked {
                    handle,
                    status,
                    diagnostic,
                    ..
                } => {
                    println!(
                        "the peer refused our {}: {status:?} {diagnostic:?}",
                        handle.kind
                    );
                    REFUSALS.fetch_add(1, Ordering::Relaxed);
                }
                other => println!("{other:?}"),
            }
        }
    }
    let _: &SessionState = cem.state();
    println!("stats: {:?}", cem.stats());
    Ok(())
}
