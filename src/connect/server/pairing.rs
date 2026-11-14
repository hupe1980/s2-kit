//! The pairing endpoints, and the per-node serialisation the specification demands.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;

use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, ConnectError, ConnectionDetails, Deployment, EndpointDescription, HmacBinding,
    NodeId, PairingAccepted, PairingAttemptId, PairingErrorMessage, PairingRefused, PairingRequest,
    PairingServer,
};
use crate::connect::tls::Fingerprint;
use crate::types::Timestamp;

use super::store::{PairedNode, PairingKey, Store};

/// What this endpoint tells a client it should connect to, when this endpoint is the
/// communication server (7A).
///
/// A template rather than a [`ConnectionDetails`]: the access token in the details is a
/// secret **per pairing** — `S2C §8A`, "The server **must** generate an access token for
/// the HTTPS client … by a cryptographically secure pseudorandom number generator" — so
/// [`PairingService`] mints a fresh one for every client and only the URL and the CA are
/// the host's to supply. Handing every client the same token lets any one of them open a
/// session as any other.
#[derive(Debug, Clone)]
pub struct CommunicationServer {
    /// The base URL of this endpoint's session-initiation API, **without** a version:
    /// `S2C §8A`, `initiateSessionUrl` "does not include the version number".
    pub initiate_session_url: String,
    /// The CA this endpoint's communication certificate is issued by, sent as
    /// `certificateFingerprint.SHA256` so a client can cross-check what it pins.
    pub ca: Fingerprint,
}

/// What a completed `finalizePairing` changed.
#[derive(Debug, Clone, Default)]
pub struct Finalized {
    /// The pairing stored, if the client reported success.
    pub paired: Option<PairingKey>,
    /// Pairings this one replaced, whose live sessions must end: the same two nodes
    /// paired before (`S2C §Pairing process`, "the current communication session should
    /// be terminated"), or — for a Resource Manager — the CEM it was paired with before,
    /// since "A RM can only be paired with one CEM at a time".
    pub displaced: Vec<PairingKey>,
}

/// Runs the pairing half of an endpoint.
///
/// One [`PairingServer`] per node, because the rate limit is per node: "Pairing attempts
/// targeting different nodes **may** be processed in parallel… a server representing
/// multiple nodes is not globally limited to one pairing attempt per second."
///
/// Serialisation is a per-node lock rather than a check-and-set, so two requests that
/// arrive in the same millisecond cannot both see an idle slot.
pub struct PairingService {
    store: Arc<dyn Store>,
    /// One state machine per node, each behind its own lock.
    attempts: std::sync::Mutex<BTreeMap<NodeId, Arc<tokio::sync::Mutex<PairingServer>>>>,
    endpoint: EndpointDescription,
    /// What the challenge response is bound to: this endpoint's leaf fingerprint in a
    /// LAN, its domain name in a WAN. Decided once, at construction, because it cannot
    /// change while the endpoint is serving.
    binding: HmacBinding,
    /// What to hand a client that will be the communication client.
    communication: Option<CommunicationServer>,
}

impl core::fmt::Debug for PairingService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PairingService")
            .field("endpoint", &self.endpoint)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl PairingService {
    /// A service for a LAN endpoint, bound to the leaf certificate it serves.
    #[must_use]
    ///
    /// The endpoint description's `deployment` is set to LAN when it says nothing: which
    /// side becomes the communication server is decided from it, and a peer reading an
    /// absent deployment cannot know.
    pub fn lan(
        store: Arc<dyn Store>,
        mut endpoint: EndpointDescription,
        leaf: Fingerprint,
    ) -> Self {
        endpoint.deployment.get_or_insert(Deployment::Lan);
        Self {
            store,
            attempts: std::sync::Mutex::new(BTreeMap::new()),
            endpoint,
            binding: leaf.binding(),
            communication: None,
        }
    }

    /// A service for a WAN endpoint, bound to its domain name.
    #[must_use]
    ///
    /// The endpoint description's `deployment` is set to WAN when it says nothing.
    pub fn wan(
        store: Arc<dyn Store>,
        mut endpoint: EndpointDescription,
        domain: impl Into<String>,
    ) -> Self {
        endpoint.deployment.get_or_insert(Deployment::Wan);
        Self {
            store,
            attempts: std::sync::Mutex::new(BTreeMap::new()),
            endpoint,
            binding: HmacBinding::DomainName(domain.into()),
            communication: None,
        }
    }

    /// Be the communication server for clients that pair with this endpoint (7A).
    ///
    /// Without this, a client that should be the communication client is refused with
    /// `IncompatibleCommunicationProtocols`.
    #[must_use]
    pub fn as_communication_server(mut self, communication: CommunicationServer) -> Self {
        self.communication = Some(communication);
        self
    }

    /// The store, for a route that needs it.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// The endpoint's own description.
    #[must_use]
    pub fn endpoint(&self) -> &EndpointDescription {
        &self.endpoint
    }

    /// Which node a request is for.
    ///
    /// `S2C-OAS`: `nodeId` when the client knows it, `nodeIdAlias` when it only has the
    /// code, neither when the endpoint hosts exactly one node — and never both.
    fn target(&self, request: &PairingRequest) -> Result<NodeId, ConnectError> {
        match (&request.node_id, &request.node_id_alias) {
            (Some(_), Some(_)) => Err(refused(PairingErrorMessage::ParsingError)),
            (Some(id), None) => self
                .store
                .node(*id)
                .map(|n| n.id)
                .ok_or_else(|| refused(PairingErrorMessage::NodeNotFound)),
            (None, Some(alias)) => self
                .store
                .resolve_alias(alias)
                .ok_or_else(|| refused(PairingErrorMessage::NodeNotFound)),
            (None, None) => self
                .store
                .sole_node()
                .ok_or_else(|| refused(PairingErrorMessage::NoNodeIdProvided)),
        }
    }

    fn machine(
        &self,
        node: NodeId,
    ) -> Result<Arc<tokio::sync::Mutex<PairingServer>>, ConnectError> {
        let description = self
            .store
            .node(node)
            .ok_or_else(|| refused(PairingErrorMessage::NodeNotFound))?;
        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| refused(PairingErrorMessage::Other))?;
        Ok(attempts
            .entry(node)
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(PairingServer::new(
                    description,
                    self.endpoint.clone(),
                    self.binding.clone(),
                )))
            })
            .clone())
    }

    /// `POST /v1/requestPairing`.
    ///
    /// Returns the body to send and the instant before which it must not be sent — the
    /// caller sleeps until then, because the delay is the specification's brute-force
    /// mitigation and a server that computes it and answers anyway has implemented a
    /// comment.
    ///
    /// "One attempt per second per node" is enforced by the state machine rather than by
    /// holding a lock across the wait: [`PairingServer`] remembers the attempt in flight
    /// and when the last one finished, so a second request is
    /// [`ConnectError::RateLimited`] whether or not the first has finished sleeping. The
    /// `try_lock` here is only about two requests reaching the *same* machine in the same
    /// instant.
    pub fn request_pairing(
        &self,
        request: &PairingRequest,
        server_nonce: &[u8],
        attempt_id: PairingAttemptId,
        now: Timestamp,
    ) -> Result<(NodeId, Timestamp, PairingAccepted), ConnectError> {
        let node = self.target(request)?;
        let token = self
            .store
            .pairing_token(node, now)
            .ok_or_else(|| refused(PairingErrorMessage::NoValidPairingTokenOnPairingServer))?;

        let machine = self.machine(node)?;
        // `try_lock`, not `lock`: a second attempt for this node must be told to come back
        // rather than queued behind a fifteen-second budget.
        let Ok(mut guard) = machine.try_lock() else {
            return Err(ConnectError::RateLimited);
        };
        let (send_after, accepted) =
            guard.request_pairing(request, &token, server_nonce, attempt_id, now)?;
        Ok((node, send_after, accepted))
    }

    /// `POST /v1/requestConnectionDetails` (6A) — this endpoint is the communication
    /// server.
    pub async fn request_connection_details(
        &self,
        node: NodeId,
        bearer: &str,
        response: &str,
        now: Timestamp,
    ) -> Result<ConnectionDetails, ConnectError> {
        let machine = self.machine(node)?;
        let mut guard = machine.lock().await;
        if !guard.authenticates(bearer) {
            return Err(ConnectError::NotPaired);
        }
        guard.verify_client(response, now)?;
        let communication = self
            .communication
            .as_ref()
            .ok_or_else(|| refused(PairingErrorMessage::IncompatibleCommunicationProtocols))?;
        let mut entropy = [0u8; 32];
        super::fill_random(&mut entropy).map_err(|_| refused(PairingErrorMessage::Other))?;
        let details = ConnectionDetails::new(
            communication.initiate_session_url.clone(),
            AccessToken::from_entropy(&entropy)?,
        )
        .with_ca_fingerprint(communication.ca.as_bytes());
        guard.connection_details(details, now)
    }

    /// `POST /v1/postConnectionDetails` (6B) — the *client* is the communication server.
    pub async fn post_connection_details(
        &self,
        node: NodeId,
        bearer: &str,
        response: &str,
        details: ConnectionDetails,
        now: Timestamp,
    ) -> Result<(), ConnectError> {
        let machine = self.machine(node)?;
        let mut guard = machine.lock().await;
        if !guard.authenticates(bearer) {
            return Err(ConnectError::NotPaired);
        }
        guard.verify_client(response, now)?;
        guard.accept_connection_details(details, now)
    }

    /// `POST /v1/finalizePairing`.
    pub async fn finalize(
        &self,
        node: NodeId,
        bearer: &str,
        success: bool,
        now: Timestamp,
    ) -> Result<Finalized, ConnectError> {
        let machine = self.machine(node)?;
        let mut guard = machine.lock().await;
        if !guard.authenticates(bearer) {
            return Err(ConnectError::NotPaired);
        }
        let pairing = guard.finalize(crate::connect::proto::FinalizePairing { success }, now)?;
        drop(guard);

        let mut finalized = Finalized::default();
        let Some(pairing) = pairing else {
            return Ok(finalized);
        };
        let key = PairingKey {
            local: pairing.local,
            remote: pairing.remote.id,
        };
        // Re-pairing the same two nodes replaces the old pairing, and its session with it.
        if self.store.paired(key).is_some() {
            finalized.displaced.push(key);
        }
        let local_role = self.store.node(pairing.local).map(|n| n.role);
        self.store.pair(PairedNode {
            local: pairing.local,
            remote: pairing.remote.clone(),
            tokens: crate::connect::proto::TokenStore::new(pairing.details.access_token.clone()),
            details: Some(pairing.details),
            revoked: false,
        });
        finalized.paired = Some(key);
        // `S2C §Pairing process`: "A CEM can be paired with multiple RMs at the same
        // time. A RM can only be paired with one CEM at a time." So when the node that was
        // just paired is a Resource Manager, whatever CEM it was paired with before is
        // dropped — *after* the new pairing is stored, so a failure in between leaves the
        // device reachable rather than orphaned.
        if local_role == Some(crate::connect::proto::Role::Rm) {
            for previous in self.store.pairings_of(pairing.local) {
                if previous != key {
                    crate::trace::event!(
                        info,
                        node = %pairing.local,
                        unpaired = %previous.remote,
                        "a Resource Manager pairs with one CEM at a time"
                    );
                    self.store.unpair(previous);
                    finalized.displaced.push(previous);
                }
            }
        }
        Ok(finalized)
    }
}

fn refused(message: PairingErrorMessage) -> ConnectError {
    ConnectError::Refused(PairingRefused {
        error_message: message,
        additional_info: None,
    })
}
