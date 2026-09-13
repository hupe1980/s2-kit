//! The HTTPS half of S2 Connect: the driver that carries [`proto`](super::proto) over the
//! network.
//!
//! There is deliberately almost no logic here. Every rule — the fifteen-second budgets,
//! the order of the challenge–response, which side supplies the connection details, when
//! a token may be retired — lives in the state machine, and this module's whole job is to
//! turn each of its outputs into a request and each response back into an input. If you
//! find yourself wanting to add a decision to this file, it belongs one layer down.
//!
//! ```text
//!   proto::PairingClient          client::Pairing            the network
//!   ────────────────────          ──────────────            ───────────
//!   request_pairing(nonce)   →    POST requestPairing    →
//!   accept(body, …)          ←    200 / 400 / 503        ←
//!   → NextStep::…            →    POST …ConnectionDetails →
//!   finalize()               →    POST finalizePairing    →
//! ```
//!
//! # The one thing this layer does decide
//!
//! It decides *when the TLS binding is known*, because only it has a socket. In a LAN the
//! `F` of `R = HMAC(C, T ‖ F)` is the fingerprint of the certificate the server presented,
//! which does not exist until the first request has been made. So [`Pairing::run`] makes
//! request 1 under [`TlsPolicy::LanPairingOnly`](super::tls::TlsPolicy::LanPairingOnly),
//! reads the fingerprint back out of the handshake, and only then hands the state machine
//! something to verify. That ordering is the reason the module exists.

mod http;
mod lan;
mod pairing;
mod session;

pub use http::{Error, Status};
pub use lan::LanClient;
pub use pairing::{Paired, Pairing};
pub use session::Session;
