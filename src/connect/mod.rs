//! S2 Connect: discovery, pairing, session initiation and the transport.
//!
//! S2 itself says nothing about how its messages travel — the standard is deliberately a
//! *semantic* protocol, and Victron's Venus OS carries the same JSON over D-Bus.
//! **S2 Connect 1.0.0** is the specification that makes it work over IP: how two nodes
//! find each other, how a person's trust in both of them is turned into a shared secret,
//! and how that secret becomes a WebSocket.
//!
//! # The shape of it
//!
//! ```text
//!  discovery          pairing                    session initiation        S2
//!  ─────────          ───────                    ──────────────────        ──
//!  _s2connect._tcp    requestPairing             initiateSession           wss://
//!  or a typed URL     → challenge/response       → pending accessToken     Authorization:
//!                     requestConnectionDetails   confirmAccessToken          Bearer …
//!                     or postConnectionDetails   → websocketUrl + token
//!                     finalizePairing
//! ```
//!
//! # What lives where
//!
//! [`proto`] is the whole protocol as pure state machines: no sockets, no clock, no
//! randomness it did not receive. That is where the parts that are easy to get wrong
//! live — the HMAC binding, the fifteen-second budgets, the one-attempt-per-node-per-
//! second rate limit, the pending-token dance that must not lose a pairing if the
//! network drops between two requests.
//!
//! Putting them there is a deliberate answer to how these rules get lost: the official
//! Rust implementation carries its rate limit as an open issue, because a rate limit that
//! lives in a request handler is one a request handler can forget. A state machine that
//! refuses to advance cannot.
//!
//! # Roles are not what you expect
//!
//! The HTTPS client/server roles of pairing are independent of the WebSocket
//! client/server roles of communication, and both are independent of CEM and RM.
//! `S2C §Mapping the CEM and RM to communication server or client`: a WAN node is always
//! the communication server; when both are LAN or both WAN, the CEM is. "A device
//! developed solely for use as an RM in a LAN setup will never function as a
//! communication server."

pub mod proto;

#[cfg(any(feature = "connect-client", feature = "connect-server"))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "connect-client", feature = "connect-server")))
)]
pub mod tls;

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
pub mod client;

#[cfg(feature = "connect-server")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-server")))]
pub mod server;

#[cfg(feature = "discovery")]
#[cfg_attr(docsrs, doc(cfg(feature = "discovery")))]
pub mod discovery;

pub use proto::{
    AccessToken, Backoff, ChallengeResponse, CommunicationToken, ConnectError, ConnectionDetails,
    Deployment, EndpointDescription, HmacBinding, HmacHashingAlgorithm, NodeDescription, NodeId,
    NodeIdAlias, Pairing, PairingClient, PairingCode, PairingServer, PairingStep, PairingToken,
    Role, SessionCredentials, SessionInitClient, SessionInitServer, TokenKind, TokenStore,
};
