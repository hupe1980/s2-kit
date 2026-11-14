//! The HTTPS endpoint an S2 Connect node exposes.
//!
//! As with [`client`](super::client), almost nothing is decided here. The router turns a
//! request into a call on [`proto::PairingServer`](crate::connect::proto::PairingServer)
//! or [`proto::SessionInitServer`](crate::connect::proto::SessionInitServer), and turns
//! that call's result back into a status code. [`SessionService`] builds its
//! `SessionInitServer` per request from the [`Store`], which works because everything that
//! must survive a restart is in the store — the grant waiting to be confirmed included.
//!
//! The status-code mapping is the interesting part, because the specification
//! distinguishes codes that are easy to collapse:
//!
//! | Outcome | Code | Why it matters |
//! |---|---|---|
//! | the attempt id is unknown | `401` | the client may **restart** the pairing |
//! | the challenge response is wrong | `403` | the attempt is **dead**; restarting is the only option |
//! | another attempt for this node is in flight | `503` | "try again soon" — not a failure |
//! | the roles or versions do not fit | `400` + body | a named reason the user can act on |
//!
//! A server that answers `400` to all four tells a client nothing, and the client's
//! recovery logic — which the specification spells out — cannot run.
//!
//! # What the host still owns
//!
//! Storage. This module keeps pairings in memory because a library cannot know where a
//! device puts its state; [`Store`] is the seam, and an implementation that writes to
//! SQLite or a flash partition drops straight in. Nothing else about the endpoint is
//! pluggable, because nothing else about it is a choice.

mod longpoll;
mod pairing;
mod routes;
mod session;
mod store;
mod subnet;
mod tls;
mod tls_error;
mod ws;

pub use longpoll::WaitingNode;
pub use pairing::{CommunicationServer, Finalized, PairingService};
pub use routes::{Endpoint, MAX_BODY_BYTES, PrepareSignal, lan_router, router};
pub use session::{LiveSession, SessionService};
pub use store::{DYNAMIC_TOKEN_LIFETIME, Memory, PairedNode, PairingKey, Store};
pub use subnet::{LocalSubnets, same_subnet};
pub use tls::{HANDSHAKE_TIMEOUT, MAX_CONNECTIONS, serve};
pub use tls_error::Error;
pub use ws::{ServerSocket, websocket_router};

/// Fill a buffer from the operating system's CSPRNG.
///
/// Every secret this endpoint mints — attempt ids, challenges, access tokens,
/// communication tokens — comes from here. A failure means the system has no entropy
/// source, which is not something to paper over with a weaker generator.
pub(crate) fn fill_random(buffer: &mut [u8]) -> Result<(), rand::rngs::SysError> {
    use rand::TryRng as _;
    rand::rngs::SysRng.try_fill_bytes(buffer)
}
