//! The operations only LAN endpoints implement.
//!
//! The wire types and the reasoning are in [`proto::lan`](crate::connect::proto::lan);
//! this is the HTTP that carries them.

use alloc::vec::Vec;

use crate::connect::proto::{
    CancelPreparePairing, EndpointDescription, LONG_POLL_TIMEOUT, NodeDescription, PreparePairing,
    WaitForPairing, WaitInstruction,
};
use crate::connect::tls::TlsPolicy;

use super::http::{Error, Http};

/// Asks a LAN endpoint what it is, before anyone has decided to pair with it.
#[derive(Debug)]
pub struct LanClient {
    http: Http,
    /// A second connection, for the one operation that is meant to block.
    ///
    /// `waitForPairing` is held open by the server for up to
    /// [`LONG_POLL_HOLD`](crate::connect::proto::LONG_POLL_HOLD); a client whose timeout
    /// is the ordinary one would abandon every request a fraction before the answer it
    /// asked for arrived, and would look to its user like an endpoint that never responds.
    long_poll: Http,
}

impl LanClient {
    /// Point at an endpoint's versioned base URL.
    ///
    /// The certificate is self-signed and unverified, exactly as during pairing — and
    /// nothing this client returns is authenticated. It is material for a user interface,
    /// not a basis for a decision. See [`proto::lan`](crate::connect::proto::lan).
    pub fn new(base_url: &str) -> Result<Self, Error> {
        Ok(Self {
            http: Http::new(
                base_url,
                &TlsPolicy::LanPairingOnly,
                super::Pairing::REQUEST_TIMEOUT,
            )?,
            long_poll: Http::new(
                base_url,
                &TlsPolicy::LanPairingOnly,
                core::time::Duration::from(LONG_POLL_TIMEOUT),
            )?,
        })
    }

    /// `GET /v1/endpoint` — the endpoint's own description.
    pub async fn endpoint(&self) -> Result<EndpointDescription, Error> {
        self.http.get("getEndpoint", "endpoint").await
    }

    /// `GET /v1/nodes` — every node this endpoint hosts.
    ///
    /// Note that the schema returns bare [`NodeDescription`]s, with no
    /// [`NodeIdAlias`](crate::connect::proto::NodeIdAlias) — so a user interface can show
    /// a list but cannot tell which entry a pairing code's alias names
    /// (erratum E21).
    pub async fn nodes(&self) -> Result<Vec<NodeDescription>, Error> {
        self.http.get("getNodes", "nodes").await
    }

    /// `POST /v1/preparePairing` — "the end user has started the process on the client".
    ///
    /// A hint that lets the server show the pairing code at the moment the user is looking
    /// for it. Best effort by design: "When a preparePairing is called, it is not
    /// guaranteed that a call to pairingRequest or cancelPreparePairing will follow."
    pub async fn prepare_pairing(&self, body: &PreparePairing) -> Result<(), Error> {
        self.http
            .post_empty("preparePairing", "preparePairing", None, Some(body))
            .await
    }

    /// `POST /v1/cancelPreparePairing` — the user changed their mind.
    pub async fn cancel_prepare_pairing(&self, body: &CancelPreparePairing) -> Result<(), Error> {
        self.http
            .post_empty(
                "cancelPreparePairing",
                "cancelPreparePairing",
                None,
                Some(body),
            )
            .await
    }

    /// `POST /v1/waitForPairing` — hold a request open until the endpoint has something
    /// to say.
    ///
    /// This is how a Resource Manager that is "purely an HTTPS client" — no listening
    /// socket, nothing for a CEM to dial — takes part in a pairing at all: it asks the
    /// other endpoint what to do, and the answer arrives whenever a person presses a
    /// button there. The server holds the request for up to
    /// [`LONG_POLL_HOLD`](crate::connect::proto::LONG_POLL_HOLD) and then answers with an
    /// empty list; that is not an error, it is "nothing yet, ask again".
    ///
    /// `S2C §Long-polling`: "the client can represent multiple nodes so the request body
    /// and the response contains a list", and the server "may only provide at most one
    /// item for each clientNodeId". Call it in a loop:
    ///
    /// ```no_run
    /// # use s2_kit::connect::client::LanClient;
    /// # use s2_kit::connect::proto::{WaitAction, WaitForPairing};
    /// # async fn poll(client: &LanClient, mine: Vec<WaitForPairing>) -> Result<(), Box<dyn std::error::Error>> {
    /// loop {
    ///     for instruction in client.wait_for_pairing(&mine).await? {
    ///         match instruction.action {
    ///             WaitAction::RequestPairing => { /* start `Pairing::run` now */ }
    ///             WaitAction::SendNodeDescription => { /* answer with descriptions */ }
    ///             _ => {}
    ///         }
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn wait_for_pairing(
        &self,
        nodes: &[WaitForPairing],
    ) -> Result<Vec<WaitInstruction>, Error> {
        self.long_poll
            .post("waitForPairing", "waitForPairing", None, &nodes)
            .await
    }
}
