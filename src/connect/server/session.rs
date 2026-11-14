//! The session-initiation endpoints, and the sessions they open.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, COMMUNICATION_TOKEN_LIFETIME, CommunicationDetails, CommunicationToken,
    ConnectError, SessionErrorMessage, SessionGrant, SessionInitServer, SessionRefused,
    SessionRequest, TokenStore, UnpairRequest,
};

use super::store::{PairingKey, Store};
use crate::types::{ProtocolVersion, Timestamp};

/// Runs the session-initiation half of an endpoint.
///
/// Almost everything it needs is in the [`Store`] and in the bearer the client presented,
/// including the replacement access token it issued and has not yet seen confirmed — a
/// server that held *that* in memory would retire the wrong token after a restart.
///
/// What it keeps in memory is what must **not** survive a restart: the communication
/// tokens handed out and not yet used (each valid for thirty seconds and one connection),
/// and the WebSocket sessions currently open, so that unpairing can end them.
pub struct SessionService {
    store: Arc<dyn Store>,
    websocket_url: String,
    s2_message_versions: Vec<ProtocolVersion>,
    /// Communication tokens handed out and not yet used, with the pairing each belongs to.
    ///
    /// The other half of `confirmAccessToken`: a server that returns a `websocketToken`
    /// and keeps no copy has authorised nothing. The list stays tiny by construction —
    /// each entry lives thirty seconds ([`COMMUNICATION_TOKEN_LIFETIME`]), is removed the
    /// moment it is used, and expired ones are swept whenever one is added.
    issued: std::sync::Mutex<Vec<IssuedConnection>>,
    /// Held across every read-modify-write of one pairing's tokens.
    ///
    /// `initiateSession` and `confirmAccessToken` each read the token list from the store,
    /// change it and write it back. Two of them interleaved can write back a list that
    /// still holds a token the other just retired — and `S2C §Session initiation` makes
    /// an access token usable "only once". One lock, held for microseconds and never
    /// across an `.await`, makes each rotation atomic for any [`Store`].
    rotation: std::sync::Mutex<()>,
    /// The S2 WebSocket sessions open right now, so unpairing can end them.
    live: std::sync::Mutex<Vec<LiveEntry>>,
    next_live: core::sync::atomic::AtomicU64,
}

/// A communication token this endpoint issued and has not yet seen used.
#[derive(Debug)]
struct IssuedConnection {
    key: PairingKey,
    token: CommunicationToken,
    issued: Timestamp,
}

#[derive(Debug)]
struct LiveEntry {
    key: PairingKey,
    id: u64,
    end: tokio::sync::watch::Sender<bool>,
}

/// One open S2 WebSocket session, as the endpoint tracks it.
///
/// Returned by [`SessionService::register_session`]; the session is tracked for as long
/// as this lives. [`ended`](Self::ended) resolves when the pairing is undone —
/// `S2C §Termination`: "After two nodes have unpaired, the S2 WebSocket connection
/// **MUST** be terminated immediately."
#[derive(Debug)]
pub struct LiveSession {
    key: PairingKey,
    id: u64,
    end: tokio::sync::watch::Receiver<bool>,
    service: alloc::sync::Weak<SessionService>,
}

impl LiveSession {
    /// The pairing this session belongs to.
    #[must_use]
    pub const fn key(&self) -> PairingKey {
        self.key
    }

    /// Resolves when the session must end because its pairing was undone.
    pub async fn ended(&mut self) {
        // An error means the service is gone, which ends the session too.
        let _ = self.end.wait_for(|ended| *ended).await;
    }

    /// Whether the session must end now.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        *self.end.borrow()
    }

    /// A receiver that observes the same signal, for a transport that wants to watch it.
    pub(super) fn watch(&self) -> tokio::sync::watch::Receiver<bool> {
        self.end.clone()
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        if let Some(service) = self.service.upgrade()
            && let Ok(mut live) = service.live.lock()
        {
            live.retain(|entry| entry.id != self.id);
        }
    }
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
    /// A service answering on `websocket_url`, which must be `wss://`
    /// (`S2C §Encrypted connection`: "S2 **MUST NOT** be sent over unencrypted channels").
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
            rotation: std::sync::Mutex::new(()),
            live: std::sync::Mutex::new(Vec::new()),
            next_live: core::sync::atomic::AtomicU64::new(0),
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
    fn machine(&self, key: PairingKey) -> Option<(bool, SessionInitServer)> {
        let paired = self.store.paired(key)?;
        Some((
            paired.revoked,
            SessionInitServer::new(key.local, key.remote, paired.tokens, &self.websocket_url)
                .with_s2_message_versions(self.s2_message_versions.clone()),
        ))
    }

    /// `POST /v1/initiateSession`.
    ///
    /// The replacement token is stored **alongside** the presented one, never instead of
    /// it: until the client confirms, either must work, or a lost reply destroys the
    /// pairing.
    ///
    /// A pairing this endpoint revoked answers `NoLongerPaired` — once, to a client that
    /// proves it held it — and is then forgotten.
    pub fn initiate_session(
        &self,
        bearer: &AccessToken,
        request: &SessionRequest,
        access_entropy: &[u8],
        now: Timestamp,
    ) -> Result<SessionGrant, ConnectError> {
        let key = PairingKey {
            local: request.server_node_id,
            remote: request.client_node_id,
        };
        let _rotation = self.rotation.lock().map_err(|_| ConnectError::NotPaired)?;
        let (revoked, mut machine) = self.machine(key).ok_or(ConnectError::NotPaired)?;
        if revoked {
            if !machine.authenticates(bearer) {
                return Err(ConnectError::NotPaired);
            }
            self.store.unpair(key);
            return Err(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::NoLongerPaired,
                additional_info: None,
            }));
        }
        let mut grant = machine.initiate_session(bearer, request, access_entropy, now)?;
        self.store.set_tokens(key, machine.tokens().clone());
        // The only thing the store knows and the state machine does not: who we are.
        grant.server_node_description = self.store.node(key.local);
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
    ) -> Result<(PairingKey, CommunicationDetails), ConnectError> {
        let details = {
            let _rotation = self.rotation.lock().map_err(|_| ConnectError::NotPaired)?;
            let key = self
                .store
                .pairing_for_token(bearer)
                .ok_or(ConnectError::NotPaired)?;
            let (_, mut machine) = self.machine(key).ok_or(ConnectError::NotPaired)?;
            // Refuses anything that is not a grant this endpoint issued, and any grant
            // older than `PENDING_TOKEN_LIFETIME` — leaving, in both cases, the tokens the
            // client already had in force.
            let details = machine.confirm_access_token(bearer, communication_entropy, now)?;
            self.store.set_tokens(key, machine.tokens().clone());
            (key, details)
        };
        let (key, details) = details;

        let CommunicationDetails::WebSocket {
            websocket_token, ..
        } = &details;
        if let Ok(mut issued) = self.issued.lock() {
            issued.retain(|entry| {
                now.saturating_duration_since(entry.issued) < COMMUNICATION_TOKEN_LIFETIME
                    && entry.key != key
            });
            issued.push(IssuedConnection {
                key,
                token: websocket_token.clone(),
                issued: now,
            });
        }
        Ok((key, details))
    }

    /// Whether a bearer presented on a WebSocket upgrade is one this endpoint issued, and
    /// which pairing it belongs to.
    ///
    /// The token is consumed: `s2-connect-common.yml/CommunicationToken` is "valid for a
    /// single connection … for maximum 30 seconds", so a replay is refused rather than
    /// granted a second session in parallel with the first. Every token is compared in
    /// constant time and the list is swept of expired entries on the way through.
    ///
    /// This is what a host mounts its `/s2` route on —
    /// [`websocket_router`](super::websocket_router) does it for you.
    pub fn authorize_connection(&self, presented: &str, now: Timestamp) -> Option<PairingKey> {
        let mut issued = self.issued.lock().ok()?;
        issued.retain(|entry| {
            now.saturating_duration_since(entry.issued) < COMMUNICATION_TOKEN_LIFETIME
        });
        // Every entry is examined, so the time taken does not say which one matched.
        let mut found = None;
        for (index, entry) in issued.iter().enumerate() {
            if entry.token.verify_str(presented) {
                found = Some((index, entry.key));
            }
        }
        let (index, key) = found?;
        issued.remove(index);
        Some(key)
    }

    /// How many issued communication tokens are still waiting to be used.
    ///
    /// For a health endpoint, and for a test that wants to see one consumed.
    #[must_use]
    pub fn pending_connections(&self) -> usize {
        self.issued.lock().map_or(0, |issued| issued.len())
    }

    /// Start tracking an open WebSocket session, so unpairing can end it.
    ///
    /// [`websocket_router`](super::websocket_router) does this for every socket it
    /// accepts; a host terminating the WebSocket itself calls it after
    /// [`authorize_connection`](Self::authorize_connection).
    #[must_use]
    pub fn register_session(self: &Arc<Self>, key: PairingKey) -> LiveSession {
        let id = self
            .next_live
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let (end, watch) = tokio::sync::watch::channel(false);
        if let Ok(mut live) = self.live.lock() {
            live.push(LiveEntry { key, id, end });
        }
        LiveSession {
            key,
            id,
            end: watch,
            service: Arc::downgrade(self),
        }
    }

    /// How many WebSocket sessions are open for one pairing.
    #[must_use]
    pub fn live_sessions(&self, key: PairingKey) -> usize {
        self.live
            .lock()
            .map_or(0, |live| live.iter().filter(|e| e.key == key).count())
    }

    /// End every open session of a pairing, and void its unused communication tokens.
    pub fn end_sessions(&self, key: PairingKey) {
        if let Ok(live) = self.live.lock() {
            for entry in live.iter().filter(|e| e.key == key) {
                let _ = entry.end.send(true);
            }
        }
        if let Ok(mut issued) = self.issued.lock() {
            issued.retain(|entry| entry.key != key);
        }
    }

    /// `POST /v1/unpair` — the client renounces the pairing.
    pub fn unpair(
        &self,
        bearer: &AccessToken,
        request: &UnpairRequest,
    ) -> Result<(), ConnectError> {
        let key = PairingKey {
            local: request.server_node_id,
            remote: request.client_node_id,
        };
        {
            let _rotation = self.rotation.lock().map_err(|_| ConnectError::NotPaired)?;
            let (_, mut machine) = self.machine(key).ok_or(ConnectError::NotPaired)?;
            machine.unpair(bearer, request)?;
            self.store.unpair(key);
        }
        self.end_sessions(key);
        Ok(())
    }

    /// Unpair from this side: end the pairing's sessions now, and answer its next
    /// `initiateSession` with `NoLongerPaired`.
    ///
    /// `S2C §Unpairing by the communication server` also says the server "**should**
    /// send an S2 `SessionRequest` … `RECONNECT`" over the open session first; that is a
    /// message of the S2 session the host runs, so send it before calling this.
    pub fn unpair_remote(&self, key: PairingKey) {
        self.store.revoke(key);
        self.end_sessions(key);
    }

    /// The tokens of one pairing, for a route that must check a bearer.
    #[must_use]
    pub fn tokens(&self, key: PairingKey) -> Option<TokenStore> {
        self.store.paired(key).map(|p| p.tokens)
    }
}
