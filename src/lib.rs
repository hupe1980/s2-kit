//! The S2 energy flexibility standard in Rust.
//!
//! S2 — formally **EN 50491-12-2**, on its way to becoming IEC 63402-2 — is the European
//! standard for communicating energy flexibility between a **Customer Energy Manager**
//! (CEM) and a **Resource Manager** (RM). The RM describes *how* a device can behave; the
//! CEM decides *why* it should. The standard's own image is a menu: the RM writes it, the
//! CEM orders from it, and the RM may always refuse.
//!
//! This crate implements the two open specifications that make the standard usable over
//! IP:
//!
//! * **S2 JSON v1.0.0** — the 36 messages and 41 component types, their encoding, and the
//!   protocol around them: control-type selection, reception statuses, revocation,
//!   validity windows, timers and instruction lifecycles.
//! * **S2 Connect 1.0.0** — DNS-SD discovery, HMAC challenge–response pairing,
//!   access-token session initiation, the WebSocket transport and the reconnection rules.
//!
//! # Where to start
//!
//! | You are building | Start at |
//! |---|---|
//! | A device that offers flexibility | [`session::RmSession`] |
//! | An energy manager that uses it | [`session::CemSession`] |
//! | A proxy, analyzer or log reader | [`session::Analyzer`] |
//! | Two devices that must find and trust each other | [`connect`] |
//! | Something over a transport that is not WebSocket | [`session`], and drive it yourself |
//!
//! Longer guides, the full rule catalogue and the conformance statement are at
//! <https://hupe1980.github.io/s2-kit>.
//!
//! ```
//! use s2_kit::prelude::*;
//!
//! let details = ResourceManagerDetails::builder()
//!     .resource_id(Id::parse("battery-1")?)
//!     .roles(vec![Role { role: RoleType::EnergyStorage, commodity: Commodity::Electricity }])
//!     .instruction_processing_delay(Duration::from_millis(500))
//!     .available_control_types(vec![ControlType::FillRateBasedControl])
//!     .provides_forecast(false)
//!     .provides_power_measurement_types(vec![CommodityQuantity::ElectricPower3PhaseSymmetric])
//!     .build();
//!
//! let mut rm = RmSession::new(RmConfig::default(), details);
//! rm.open(Timestamp::now());
//!
//! // Everything the session wants to say is waiting here; a driver writes it to a socket.
//! let first = rm.poll_transmit().expect("the RM speaks first");
//! assert!(first.text.contains(r#""message_type":"Handshake""#));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Design in one paragraph
//!
//! The protocol core is **sans-I/O**: no sockets, no clock, no tasks. Every engine takes
//! `now` as a parameter and is driven by `handle_*` / `poll_*`, so a two-hour timer, a
//! lost acknowledgement and a revoked constraint are all ordinary unit tests. Drivers —
//! WebSocket, HTTPS, mDNS — are Cargo features that hang off the side. The data model is
//! hand-written and *proven* against the standard's own JSON schemas in CI rather than
//! generated from a modified copy of them, and the layer above it is a
//! [rule-numbered semantic validator](validate) for everything JSON Schema cannot express.
//!
//! # Features
//!
//! | Feature | Enables |
//! |---|---|
//! | `std` *(default)* | The standard library. Without it the crate is `no_std + alloc`. |
//! | `uuid` *(default)* | [`Id::generate`](types::Id::generate) and friends. |
//! | `tokio` | [`io::Driver`], which pumps a session over any text transport. |
//! | `connect-client` | Pairing and session initiation, with leaf-fingerprint capture and CA pinning. |
//! | `connect-server` | An `axum` router for both APIs and the authorised WebSocket, TLS serving, a self-signed CA helper. |
//! | `discovery` | DNS-SD advertise and browse of `_s2connect._tcp`. |
//! | `tls-ring` *(default)* | `ring` as the bundled TLS cryptography; no `cmake` to cross-compile. |
//! | `tls-aws-lc-rs` | `aws-lc-rs` instead: FIPS-certifiable, post-quantum. Exactly one is linked. |
//! | `jiff` / `time` / `chrono` | [`Timestamp`] conversions, with the crate re-exported. |
//! | `testing` | Fixtures, the two-engine harness, the in-memory pipe, and `.s2log` transcripts read as well as written. |
//! | `schemars` | `JsonSchema` for this crate's own types, for the equivalence check. |
//! | `tracing` | Structured events from the drivers: what crossed the wire, and what was refused. |
//! | `cli` | The `s2-kit` command-line tool. |

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// Tests are allowed to panic: that is what a failing assertion is. The lints are on for
// everything else, where a panic in a driver answering a peer is a crash.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
#![cfg_attr(doctest, doc = include_str!("../README.md"))]
#![doc(html_logo_url = "https://s2standard.org/wp-content/uploads/2023/09/Logo-S2.svg")]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod codec;
pub mod message;
pub mod model;
pub mod schema;
pub mod session;
pub mod types;
pub mod validate;

pub mod connect;

pub(crate) mod trace;

// The website's guides, as doctests. Generated by `cargo xtask gen-site-examples`; it
// exists so a snippet a reader copies out of a guide is a snippet that compiles. Only
// rustdoc ever builds it.
#[cfg(doctest)]
mod site_doctests;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub mod io;

#[cfg(feature = "testing")]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;

pub use codec::{DecodeError, DecodeOptions, Strictness, decode, decode_with, encode};
pub use message::{Message, MessageKind};
pub use types::{Duration, Id, ProtocolVersion, Timestamp, WireProfile};

/// Everything a typical Resource Manager or Customer Energy Manager needs.
pub mod prelude {
    pub use crate::codec::{DecodeError, Strictness, decode, encode};
    pub use crate::message::{Message, MessageKind};
    pub use crate::session::{
        CemConfig, CemEvent, CemSession, CloseReason, Instructed, Negotiation, RmConfig, RmEvent,
        RmSession, SessionState,
    };
    pub use crate::types::common::*;
    pub use crate::types::{
        Duration, Id, ProtocolVersion, Timestamp, WireProfile, ddbc, frbc, ombc, pebc, ppbc,
    };
    pub use crate::validate::{Report, RuleId, Severity, Validate, Violation};
}

// The rule for this block: if a type from crate `X` is reachable from this crate's public
// API, `X` is re-exported here under the feature that brings it in — so a caller names the
// version this crate was built against instead of keeping a major in step by hand. A
// drifted one otherwise reports `expected MaybeTlsStream<TcpStream>, found
// MaybeTlsStream<TcpStream>`.
#[cfg(feature = "chrono")]
#[cfg_attr(docsrs, doc(cfg(feature = "chrono")))]
pub use chrono;
#[cfg(feature = "jiff")]
#[cfg_attr(docsrs, doc(cfg(feature = "jiff")))]
pub use jiff;
#[cfg(feature = "time")]
#[cfg_attr(docsrs, doc(cfg(feature = "time")))]
pub use time;
#[cfg(feature = "uuid")]
#[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
pub use uuid;

// `WebSocket::connect` returns a
// `WebSocket<MaybeTlsStream<TcpStream>>`; [`io::Dialled`] is its short spelling.
#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use tokio;
#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use tokio_tungstenite;

// `TlsPolicy::PinnedCa` carries a `CertificateDer`, `TlsClient::config` an
// `Arc<ClientConfig>`, `default_provider` an `Arc<CryptoProvider>`.
#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use rustls;

// `connect::server`'s routers are `axum::Router`s, mounted in an application's own.
#[cfg(feature = "connect-server")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-server")))]
pub use axum;

// `Discovery::daemon` hands back the `mdns_sd::ServiceDaemon` it is driving.
#[cfg(feature = "discovery")]
#[cfg_attr(docsrs, doc(cfg(feature = "discovery")))]
pub use mdns_sd;
