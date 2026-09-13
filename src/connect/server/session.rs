//! The session-initiation endpoints.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, COMMUNICATION_TOKEN_LIFETIME, CommunicationDetails, CommunicationToken,
    ConnectError, NodeId, SessionGrant, SessionInitServer, SessionRequest, TokenStore,
    UnpairRequest,
};

use super::store::Store;
use crate::types::{ProtocolVersion, Timestamp};

/// Runs the session-initiation half of an endpoint.
///
/// Almost everything it needs is in the [`Store`] and in the bearer the client presented,
/// including the replacement access token it issued and has not yet seen confirmed — a
/// server that held *that* in memory would retire the wrong token after a restart.
///
/// The one thing it keeps in memory is the set of communication tokens it has handed out
/// and not yet seen used. Those are deliberately not persisted: each is valid for thirty
/// seconds and one connection, so a server that has restarted has none that are still
/// good, and a client whose upgrade is refused simply initiates again.
pub struct SessionService {
    store: Arc<dyn Store>,
    websocket_url: String,
    s2_message_versions: Vec<ProtocolVersion>,
    /// Communication tokens handed out and not yet used, with the node each belongs to.
    ///
    /// The other half of `confirmAccessToken`: a server that returns a `websocketToken`
    /// and keeps no copy has authorised nothing. The list stays tiny by construction —
    /// each entry lives thirty seconds ([`COMMUNICATION_TOKEN_LIFETIME`]), is removed the
    /// moment it is used, and expired ones are swept whenever one is added.
    issued: std::sync::Mutex<Vec<IssuedConnection>>,
}

/// A communication token this endpoint issued and has not yet seen used.
#[derive(Debug)]
struct IssuedConnection {
    remote: NodeId,
    token: CommunicationToken,
    issued: Timestamp,
}

impl core::fmt::Debug for SessionService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionService")
            .field("websocket_url", &self.websocket_url)
            .field("s2_message_versions", &self.s2_message_versions)
            .finish_non_exhaustive()
    }
}

impl SessionService {
    /// A service answering on `websocket_url`.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, websocket_url: impl Into<String>) -> Self {
        Self {
            store,
            websocket_url: websocket_url.into(),
            s2_message_versions: alloc::vec![
                crate::types::WireProfile::V1_0_0.connect_version(),
                crate::types::WireProfile::V0_0_2Beta.connect_version(),
            ],
            issued: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Offer a different set of S2 JSON message versions, newest first.
    #[must_use]
    pub fn with_s2_message_versions(mut self, versions: Vec<ProtocolVersion>) -> Self {
        self.s2_message_versions = versions;
        self
    }

    /// The store.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// The state machine for one pairing, built from what the store holds.
    ///
    /// Per request rather than per pairing, and that is the point: everything that has to
    /// survive a restart — the tokens, including the grant waiting to be confirmed — lives
    /// in the [`Store`], so a machine built fresh from it knows everything the last one
    /// did. The alternative is a second implementation of session initiation beside
    /// [`SessionInitServer`], and two implementations of one protocol drift.
    fn machine(&self, remote: NodeId) -> Option<(NodeId, SessionInitServer)> {
        let paired = self.store.paired(remote)?;
        Some((
            paired.local,
            SessionInitServer::new(paired.local, remote, paired.tokens, &self.websocket_url)
                .with_s2_message_versions(self.s2_message_versions.clone()),
        ))
    }

    /// `POST /v1/initiateSession`.
    ///
    /// The replacement token is stored **alongside** the presented one, never instead of
    /// it: until the client confirms, either must work, or a lost reply destroys the
    /// pairing.
    pub fn initiate_session(
        &self,
        bearer: &AccessToken,
        request: &SessionRequest,
        access_entropy: &[u8],
        now: Timestamp,
    ) -> Result<SessionGrant, ConnectError> {
        let (local, mut machine) = self
            .machine(request.client_node_id)
            .ok_or(ConnectError::NotPaired)?;
        let mut grant = machine.initiate_session(bearer, request, access_entropy, now)?;
        self.store
            .set_tokens(request.client_node_id, machine.tokens().clone());
        // The only thing the store knows and the state machine does not: who we are.
        grant.server_node_description = self.store.node(local);
        Ok(grant)
    }

    /// `POST /v1/confirmAccessToken`.
    ///
    /// The bearer *is* the confirmation. Every older token is retired here, and not one
    /// request earlier.
    pub fn confirm_access_token(
        &self,
        bearer: &AccessToken,
        communication_entropy: &[u8],
        now: Timestamp,
    ) -> Result<(NodeId, CommunicationDetails), ConnectError> {
        let remote = self
            .store
            .node_for_token(bearer)
            .ok_or(ConnectError::NotPaired)?;
        let (_, mut machine) = self.machine(remote).ok_or(ConnectError::NotPaired)?;
        // Refuses anything that is not a grant this endpoint issued, and any grant older
        // than `PENDING_TOKEN_LIFETIME` — leaving, in both cases, the tokens the client
        // already had in force.
        let details = machine.confirm_access_token(bearer, communication_entropy, now)?;
        self.store.set_tokens(remote, machine.tokens().clone());

        let CommunicationDetails::WebSocket {
            websocket_token, ..
        } = &details;
        if let Ok(mut issued) = self.issued.lock() {
            issued.retain(|entry| {
                now.saturating_duration_since(entry.issued) < COMMUNICATION_TOKEN_LIFETIME
                    && entry.remote != remote
            });
            issued.push(IssuedConnection {
                remote,
                token: websocket_token.clone(),
                issued: now,
            });
        }
        Ok((remote, details))
    }

    /// Whether a bearer presented on a WebSocket upgrade is one this endpoint issued, and
    /// which node it belongs to.
    ///
    /// The token is consumed: `s2-connect-common.yml/CommunicationToken` is "valid for a
    /// single connection … for maximum 30 seconds", so a replay is refused rather than
    /// granted a second session in parallel with the first. Every token is compared in
    /// constant time and the list is swept of expired entries on the way through.
    ///
    /// This is what a host mounts its `/s2` route on —
    /// [`websocket_router`](super::websocket_router) does it for you.
    pub fn authorize_connection(&self, presented: &str, now: Timestamp) -> Option<NodeId> {
        let mut issued = self.issued.lock().ok()?;
        issued.retain(|entry| {
            now.saturating_duration_since(entry.issued) < COMMUNICATION_TOKEN_LIFETIME
        });
        // Every entry is examined, so the time taken does not say which one matched.
        let mut found = None;
        for (index, entry) in issued.iter().enumerate() {
            if entry.token.verify_str(presented) {
                found = Some((index, entry.remote));
            }
        }
        let (index, remote) = found?;
        issued.remove(index);
        Some(remote)
    }

    /// How many issued communication tokens are still waiting to be used.
    ///
    /// For a health endpoint, and for a test that wants to see one consumed.
    #[must_use]
    pub fn pending_connections(&self) -> usize {
        self.issued.lock().map_or(0, |issued| issued.len())
    }

    /// `POST /v1/unpair`.
    pub fn unpair(
        &self,
        bearer: &AccessToken,
        request: &UnpairRequest,
    ) -> Result<(), ConnectError> {
        let (_, mut machine) = self
            .machine(request.client_node_id)
            .ok_or(ConnectError::NotPaired)?;
        machine.unpair(bearer, request)?;
        self.store.unpair(request.client_node_id);
        if let Ok(mut issued) = self.issued.lock() {
            issued.retain(|entry| entry.remote != request.client_node_id);
        }
        Ok(())
    }

    /// The tokens of one pairing, for a route that must check a bearer.
    #[must_use]
    pub fn tokens(&self, remote: NodeId) -> Option<TokenStore> {
        self.store.paired(remote).map(|p| p.tokens)
    }
}
