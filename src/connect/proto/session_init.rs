//! Session initiation: turning a stored pairing into one authorised WebSocket.
//!
//! # Why this is two requests and not one
//!
//! An access token is valid **one time**. A naive exchange — client presents the token,
//! server burns it and issues a new one in the reply — loses the pairing outright if that
//! reply never arrives: the server has retired a token the client never received, and the
//! only way back is for a human to pair the devices again.
//!
//! S2 Connect solves it by making the *second* request the proof, and this module is that
//! two-phase commit:
//!
//! ```text
//!   communication client                         communication server
//!   ────────────────────                         ────────────────────
//!   POST /v1/initiateSession ────────────────▶   the presented token still works;
//!     Bearer <accessToken>                       a new one is issued as *pending*
//!     clientNodeId, serverNodeId,
//!     versions, protocols
//!                          ◀───────────────────  selectedS2MessageVersion,
//!   store the new token                          selectedCommunicationProtocol,
//!                                                accessToken (the replacement)
//!   POST /v1/confirmAccessToken ─────────────▶   the *new* token is the bearer, which
//!     Bearer <the new accessToken>               is the proof it was stored; only now
//!                          ◀───────────────────  is the old one retired
//!                                                websocketUrl, websocketToken
//!   wss:// with Bearer <websocketToken>, inside 30 seconds
//! ```
//!
//! Authenticating the confirmation *with the new token* is the whole trick: the server
//! needs no separate assertion that the client stored it, because presenting it is the
//! assertion. `S2C §confirmAccessToken`: "it is crucial that the server knows certainly
//! that client has properly stored its access token."
//!
//! Until that confirmation arrives, **both** tokens open the door. A client that crashed
//! in between retries with whichever it has — `S2C §Recovery`: "It should try all the
//! accessTokens sequentially" — and the pairing survives.
//!
//! # Two lifetimes, deliberately different
//!
//! The access token lasts up to five years and is the long-term credential; the
//! communication token lasts thirty seconds and authorises exactly one connection. If the
//! WebSocket handshake does not happen inside that half-minute, the whole initiation is
//! repeated — cheap, and far better than a bearer token loose in a log file for five
//! years.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::types::{Duration, ProtocolVersion, Timestamp, WireProfile};

use super::{
    AccessToken, CommunicationProtocol, CommunicationToken, ConnectError, EndpointDescription,
    NodeDescription, NodeId,
};

/// How long a communication token stays usable.
///
/// `s2-connect-common.yml/CommunicationToken`: "valid for maximum 30 seconds".
pub const COMMUNICATION_TOKEN_LIFETIME: Duration = Duration::from_secs(30);

/// How long a freshly granted access token may wait to be confirmed.
///
/// `S2C §Session initiation`: the server activates the replacement it issued only if the
/// confirmation arrives "not more than 15 seconds" later. After that the grant is stale —
/// the client is told to start again, and the token it already held keeps working, which
/// is what stops an interrupted rotation from destroying the pairing.
pub const PENDING_TOKEN_LIFETIME: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Wire types — `s2-connect-session-init.yml`
// ---------------------------------------------------------------------------

/// The body of `POST /v1/initiateSession`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    /// Who is asking.
    pub client_node_id: NodeId,
    /// Who it wants to talk to.
    pub server_node_id: NodeId,
    /// **S2 JSON** message versions the client speaks, newest first.
    pub supported_s2_message_versions: Vec<ProtocolVersion>,
    /// How it can carry them.
    pub supported_communication_protocols: Vec<CommunicationProtocol>,
    /// An updated description, when something about the node has changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_node_description: Option<NodeDescription>,
    /// An updated endpoint description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_endpoint_description: Option<EndpointDescription>,
}

/// The `200` body of `initiateSession`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGrant {
    /// The transport the server chose.
    pub selected_communication_protocol: CommunicationProtocol,
    /// The S2 JSON message version the server chose from the client's list.
    ///
    /// This is the same string a bare-WebSocket `Handshake` negotiates, which is why a
    /// session opened through S2 Connect uses
    /// [`Negotiation::PreNegotiated`](crate::session::Negotiation::PreNegotiated) and
    /// must not send handshake messages at all.
    pub selected_s2_message_version: ProtocolVersion,
    /// The **replacement** access token, not yet in force.
    pub access_token: AccessToken,
    /// An updated description of the server's node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_node_description: Option<NodeDescription>,
    /// An updated description of the server's endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_endpoint_description: Option<EndpointDescription>,
}

impl SessionGrant {
    /// The wire profile the chosen version selects, if this crate speaks it.
    #[must_use]
    pub fn profile(&self) -> Option<WireProfile> {
        // Through `ProtocolVersion`, not a literal comparison: `S2C` spells this version
        // `v1.0.0` and S2 JSON spells it `1.0.0`, and a server that follows its own
        // specification must not read as "a version this crate does not speak" (E25).
        self.selected_s2_message_version.wire_profile()
    }
}

/// The `200` body of `confirmAccessToken`: where to connect, and with what.
///
/// `S2C-OAS session-init/WebSocketCommunicationDetails`. The discriminator is
/// `communicationProtocol`; there is one variant today.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "communicationProtocol")]
#[non_exhaustive]
pub enum CommunicationDetails {
    /// A WebSocket.
    WebSocket {
        /// Where to connect. `wss://`, always.
        #[serde(rename = "websocketUrl")]
        websocket_url: String,
        /// The bearer for that one connection.
        ///
        /// The prose calls this `commToken` and the schema calls it `websocketToken`
        /// (erratum E9); they are the same thing.
        #[serde(rename = "websocketToken")]
        websocket_token: CommunicationToken,
    },
}

/// Why a session could not be initiated.
/// `S2C-OAS session-init/CommunicationDetailsErrorMessage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SessionErrorMessage {
    /// No S2 JSON message version in common.
    IncompatibleS2MessageVersions,
    /// No transport in common.
    IncompatibleCommunicationProtocols,
    /// The pairing is gone; a human must pair the devices again.
    NoLongerPaired,
    /// The body was not understood.
    ParsingError,
    /// Anything else.
    Other,
}

/// The `400` body of `initiateSession`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRefused {
    /// Which of the enumerated reasons.
    pub error_message: SessionErrorMessage,
    /// Free text, for a log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_info: Option<String>,
}

/// The body of `POST /v1/unpair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpairRequest {
    /// Who is asking.
    pub client_node_id: NodeId,
    /// Who to forget.
    pub server_node_id: NodeId,
}

/// A grant with the clock attached: what a driver actually needs to connect.
#[derive(Debug, Clone)]
pub struct SessionCredentials {
    /// Where to connect.
    pub websocket_url: String,
    /// What to put in `Authorization: Bearer …`.
    pub websocket_token: CommunicationToken,
    /// The S2 JSON message version both sides settled on.
    pub profile: Option<WireProfile>,
    /// When it was issued.
    pub issued: Timestamp,
}

impl SessionCredentials {
    /// The moment after which the token will be refused.
    #[must_use]
    pub fn expires(&self) -> Timestamp {
        self.issued
            .checked_add(COMMUNICATION_TOKEN_LIFETIME)
            .unwrap_or(self.issued)
    }

    /// Whether the thirty seconds have run out.
    ///
    /// Worth asking before dialling rather than after: a driver that queues a connection
    /// behind a slow DNS lookup can easily arrive too late, and the failure a server
    /// returns for an expired token looks exactly like the one it returns for a wrong one.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires()
    }
}

// ---------------------------------------------------------------------------
// The token store
// ---------------------------------------------------------------------------

/// One access token the store holds, and when it was granted.
#[derive(Debug, Clone)]
struct Held {
    token: AccessToken,
    /// `None` for a token that is already in force — the one pairing produced, or one a
    /// confirmation has activated. `Some(t)` for a replacement granted at `t` and not yet
    /// confirmed, which is the only kind [`PENDING_TOKEN_LIFETIME`] applies to.
    granted: Option<Timestamp>,
}

/// The access tokens a paired node holds, newest first, and the rules that govern them.
///
/// This is the piece that makes the pairing survivable. Storing a *list* rather than one
/// token is why a dropped reply is a retry instead of a service call — and the list is
/// what `S2C §Recovery` asks for: "It now has (at least) two accessTokens in its list, and
/// does not know for certain which one is active at the communication server. It should
/// try all the accessTokens sequentially."
///
/// Each entry remembers whether it is already in force or is a grant still waiting to be
/// confirmed, because only the second kind expires — after
/// [`PENDING_TOKEN_LIFETIME`], which is what stops a confirmation that arrives an hour
/// late from retiring a token that still works.
#[derive(Debug, Clone, Default)]
pub struct TokenStore {
    /// Newest first. Never more than [`Self::MAX`].
    tokens: Vec<Held>,
}

impl TokenStore {
    /// How many tokens are kept before the oldest is dropped.
    ///
    /// Each failed round trip adds one. Four is well past the two the protocol produces
    /// in the worst ordinary case, and bounds what a peer can make us store.
    pub const MAX: usize = 4;

    /// A store holding the token pairing produced.
    #[must_use]
    pub fn new(current: AccessToken) -> Self {
        Self {
            tokens: alloc::vec![Held {
                token: current,
                granted: None,
            }],
        }
    }

    /// Whether a presented token opens the door.
    ///
    /// Any token in the list is accepted, because the client may hold any of them and the
    /// alternative to accepting it is a dead pairing.
    #[must_use]
    pub fn accepts(&self, presented: &AccessToken) -> bool {
        self.find(presented).is_some()
    }

    fn find(&self, presented: &AccessToken) -> Option<&Held> {
        self.tokens.iter().find(|h| h.token.verify(presented))
    }

    /// Record a replacement granted at `now` and not yet confirmed.
    ///
    /// It joins the list rather than replacing it, because until the client proves it has
    /// stored the new token both must open the door (`S2C §Recovery`). What `now` buys is
    /// the other half of the rule: [`confirm`](Self::confirm) will not activate a grant
    /// the client sat on for more than [`PENDING_TOKEN_LIFETIME`].
    pub fn add(&mut self, token: AccessToken, now: Timestamp) {
        if self.tokens.iter().any(|h| h.token.verify(&token)) {
            return;
        }
        // Retire grants that can no longer be confirmed, *before* making room. A grant
        // past [`PENDING_TOKEN_LIFETIME`] can never be activated — `confirm` refuses it —
        // so it is a slot holding nothing, and the list is only four long. Without this,
        // four `initiateSession` calls whose replies never reached the client would push
        // the token that actually works off the end, and the pairing would be dead while
        // every party still believed in it. That is the exact failure `S2C §Recovery`
        // keeps a list to avoid.
        self.tokens.retain(|held| {
            held.granted.is_none_or(|granted| {
                now.saturating_duration_since(granted) <= PENDING_TOKEN_LIFETIME
            })
        });
        self.tokens.insert(
            0,
            Held {
                token,
                granted: Some(now),
            },
        );
        self.tokens.truncate(Self::MAX);
    }

    /// Record a token that is already in force, such as the one pairing produced.
    pub fn adopt(&mut self, token: AccessToken) {
        if self.tokens.iter().any(|h| h.token.verify(&token)) {
            return;
        }
        self.tokens.insert(
            0,
            Held {
                token,
                granted: None,
            },
        );
        self.tokens.truncate(Self::MAX);
    }

    /// Activate a granted token and retire every other, as `confirmAccessToken` does.
    ///
    /// Called when the client proves it holds one — by presenting it as the bearer of
    /// `confirmAccessToken` — which is what makes retiring the others safe.
    ///
    /// # Errors
    ///
    /// [`ConnectError::NotPaired`] when the token is not one this store holds;
    /// [`ConnectError::Expired`] when it was granted more than
    /// [`PENDING_TOKEN_LIFETIME`] ago and so is no longer confirmable. An expired grant
    /// leaves the store untouched, so whatever the client held before still works.
    pub fn confirm(&mut self, token: &AccessToken, now: Timestamp) -> Result<(), ConnectError> {
        let held = self.find(token).ok_or(ConnectError::NotPaired)?;
        if let Some(granted) = held.granted
            && now.saturating_duration_since(granted) > PENDING_TOKEN_LIFETIME
        {
            return Err(ConnectError::Expired {
                what: "access token confirmation",
                limit: PENDING_TOKEN_LIFETIME,
            });
        }
        self.tokens.clear();
        self.tokens.push(Held {
            token: token.clone(),
            granted: None,
        });
        Ok(())
    }

    /// Whether a presented token is a **grant** this store issued and has not yet seen
    /// confirmed.
    ///
    /// `confirmAccessToken` must be authenticated with the *replacement*, not with any
    /// token the store happens to accept: presenting the replacement is the proof the
    /// client stored it, and that proof is the only reason retiring the others is safe.
    #[must_use]
    pub fn is_pending(&self, presented: &AccessToken) -> bool {
        self.find(presented).is_some_and(|h| h.granted.is_some())
    }

    /// The token to present next: the newest.
    #[must_use]
    pub fn current(&self) -> Option<&AccessToken> {
        self.tokens.first().map(|h| &h.token)
    }

    /// Every token to try, newest first.
    ///
    /// A client that got a `401` works down this list before concluding it is unpaired.
    pub fn candidates(&self) -> impl Iterator<Item = &AccessToken> {
        self.tokens.iter().map(|h| &h.token)
    }

    /// How many are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Whether the pairing has no token at all, which means it is gone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Forget everything. What a `NoLongerPaired` means.
    pub fn clear(&mut self) {
        self.tokens.clear();
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    AwaitingGrant,
    Granted,
    Connected,
    Failed,
}

impl Phase {
    const fn name(self) -> &'static str {
        match self {
            Phase::Start => "the start of session initiation",
            Phase::AwaitingGrant => "the session grant",
            Phase::Granted => "confirmation of the new access token",
            Phase::Connected => "a completed initiation",
            Phase::Failed => "a failed initiation",
        }
    }
}

/// The communication client's half of session initiation.
///
/// ```
/// use s2_kit::connect::proto::{AccessToken, NodeId, SessionInitClient, TokenStore};
///
/// let store = TokenStore::new(AccessToken::from_entropy(&[1u8; 32])?);
/// let mut client = SessionInitClient::new(NodeId::parse("rm-1")?, NodeId::parse("cem-1")?, store);
///
/// let (bearer, body) = client.initiate()?;
/// assert_eq!(body.client_node_id.to_string(), "rm-1");
/// # let _ = bearer;
/// # Ok::<(), Box<dyn core::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct SessionInitClient {
    client_node_id: NodeId,
    server_node_id: NodeId,
    tokens: TokenStore,
    s2_message_versions: Vec<ProtocolVersion>,
    protocols: Vec<CommunicationProtocol>,
    phase: Phase,
    /// The token presented on the request in flight, so a `401` can move to the next.
    attempted: Option<AccessToken>,
    /// How many tokens of the store have been tried since the last success.
    tried: usize,
    granted: Option<SessionGrant>,
}

impl SessionInitClient {
    /// A client for one pairing.
    #[must_use]
    pub fn new(client_node_id: NodeId, server_node_id: NodeId, tokens: TokenStore) -> Self {
        Self {
            client_node_id,
            server_node_id,
            tokens,
            // The `v`-prefixed spelling, because `S2C §Versioning of JSON Schema
            // files` requires it over S2 Connect. Matching is canonical, so a peer that
            // offers the bare form is still understood (E25).
            s2_message_versions: alloc::vec![
                WireProfile::V1_0_0.connect_version(),
                WireProfile::V0_0_2Beta.connect_version(),
            ],
            protocols: alloc::vec![CommunicationProtocol::WebSocket],
            phase: Phase::Start,
            attempted: None,
            tried: 0,
            granted: None,
        }
    }

    /// Offer a different set of S2 JSON message versions, newest first.
    #[must_use]
    pub fn with_s2_message_versions(mut self, versions: Vec<ProtocolVersion>) -> Self {
        self.s2_message_versions = versions;
        self
    }

    /// Build `initiateSession`, with the bearer to send it under.
    ///
    /// A completed initiation is a legitimate place to start another: every reconnection
    /// needs fresh credentials, because a communication token authorises exactly one
    /// connection and lives thirty seconds. Requiring [`Self::restart`] in between would
    /// make the common path the one a caller forgets.
    pub fn initiate(&mut self) -> Result<(AccessToken, SessionRequest), ConnectError> {
        if self.phase == Phase::Connected {
            self.phase = Phase::Start;
        }
        self.expect(Phase::Start)?;
        let token = self
            .tokens
            .candidates()
            .nth(self.tried)
            .cloned()
            .ok_or(ConnectError::NotPaired)?;
        self.attempted = Some(token.clone());
        self.phase = Phase::AwaitingGrant;
        Ok((
            token,
            SessionRequest {
                client_node_id: self.client_node_id,
                server_node_id: self.server_node_id,
                supported_s2_message_versions: self.s2_message_versions.clone(),
                supported_communication_protocols: self.protocols.clone(),
                client_node_description: None,
                client_endpoint_description: None,
            },
        ))
    }

    /// Handle a `401` on `initiateSession` by moving to the next stored token.
    ///
    /// `S2C §Recovery`: "It should try all the accessTokens sequentially." When they are
    /// exhausted the pairing really is gone, and this returns [`ConnectError::NotPaired`]
    /// — which is a call to a human, not a retry.
    pub fn unauthorized(&mut self) -> Result<(), ConnectError> {
        self.tried += 1;
        self.phase = Phase::Start;
        if self.tried >= self.tokens.len() {
            self.phase = Phase::Failed;
            return Err(ConnectError::NotPaired);
        }
        Ok(())
    }

    /// Take the grant and store the replacement token.
    ///
    /// Storing before confirming is the order the specification requires, and the reason
    /// the two requests exist at all: a client that confirmed first and stored afterwards
    /// has a window in which a crash loses the replacement for good.
    ///
    /// `now` starts the [`PENDING_TOKEN_LIFETIME`] clock on this side too, so a client
    /// that was suspended between the two requests declines to retire the token it
    /// already had rather than retiring it against a server that never activated the
    /// replacement.
    pub fn accept(&mut self, grant: SessionGrant, now: Timestamp) -> Result<(), ConnectError> {
        self.expect(Phase::AwaitingGrant)?;
        if !self
            .protocols
            .contains(&grant.selected_communication_protocol)
        {
            self.phase = Phase::Failed;
            return Err(ConnectError::NoOverlap("communication protocol"));
        }
        if !self
            .s2_message_versions
            .iter()
            .any(|v| v.matches(&grant.selected_s2_message_version))
        {
            self.phase = Phase::Failed;
            return Err(ConnectError::NoOverlap("S2 message version"));
        }
        self.tokens.add(grant.access_token.clone(), now);
        self.granted = Some(grant);
        self.phase = Phase::Granted;
        Ok(())
    }

    /// The bearer for `confirmAccessToken`: the **new** token.
    ///
    /// Presenting it *is* the confirmation, which is why there is no body.
    pub fn confirm(&mut self) -> Result<AccessToken, ConnectError> {
        self.expect(Phase::Granted)?;
        let grant = self.granted.as_ref().ok_or(ConnectError::OutOfOrder {
            got: "a confirmation",
            expected: Phase::AwaitingGrant.name(),
        })?;
        Ok(grant.access_token.clone())
    }

    /// Take the communication details the confirmation returned.
    ///
    /// The old tokens are retired here and not before: the server has just proved it
    /// accepted the new one.
    pub fn connected(
        &mut self,
        details: CommunicationDetails,
        now: Timestamp,
    ) -> Result<SessionCredentials, ConnectError> {
        self.expect(Phase::Granted)?;
        let grant = self.granted.take().ok_or(ConnectError::OutOfOrder {
            got: "communication details",
            expected: Phase::AwaitingGrant.name(),
        })?;
        self.tokens.confirm(&grant.access_token, now)?;
        self.tried = 0;
        self.phase = Phase::Connected;
        let CommunicationDetails::WebSocket {
            websocket_url,
            websocket_token,
        } = details;
        Ok(SessionCredentials {
            websocket_url,
            websocket_token,
            profile: grant.profile(),
            issued: now,
        })
    }

    /// Start again after a failure, keeping whatever tokens survived.
    ///
    /// This is what makes the two-phase commit worth having: after any failure at all, the
    /// store still holds a token the server will accept.
    pub fn restart(&mut self) {
        self.phase = Phase::Start;
        self.attempted = None;
    }

    /// Forget the pairing. What a `NoLongerPaired` means.
    pub fn unpaired(&mut self) -> UnpairRequest {
        self.tokens.clear();
        self.phase = Phase::Failed;
        UnpairRequest {
            client_node_id: self.client_node_id,
            server_node_id: self.server_node_id,
        }
    }

    /// The tokens, to persist.
    #[must_use]
    pub fn tokens(&self) -> &TokenStore {
        &self.tokens
    }

    fn expect(&mut self, phase: Phase) -> Result<(), ConnectError> {
        if self.phase == phase {
            return Ok(());
        }
        let got = self.phase.name();
        self.phase = Phase::Failed;
        Err(ConnectError::OutOfOrder {
            got,
            expected: phase.name(),
        })
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// The communication server's half of session initiation.
///
/// It owns the token store for one pairing and never hands out a communication token
/// without having accepted an access token first.
#[derive(Debug)]
pub struct SessionInitServer {
    node_id: NodeId,
    peer_node_id: NodeId,
    tokens: TokenStore,
    websocket_url: String,
    s2_message_versions: Vec<ProtocolVersion>,
    protocols: Vec<CommunicationProtocol>,
    /// The communication token the last confirmation handed out, until it is used or its
    /// thirty seconds run out.
    ///
    /// The only state here that is *not* in the [`TokenStore`], and deliberately so: it
    /// lives thirty seconds and authorises one connection, so a server that has restarted
    /// holds none that are still good. The pending **access** token is in the store,
    /// because that one must survive a restart — a server that kept it in memory would
    /// refuse the confirmation of a grant it had just issued.
    connection: Option<IssuedConnection>,
}

/// A communication token this server issued and has not yet seen used.
#[derive(Debug, Clone)]
struct IssuedConnection {
    token: CommunicationToken,
    issued: Timestamp,
}

impl SessionInitServer {
    /// A server for one pairing.
    #[must_use]
    pub fn new(
        node_id: NodeId,
        peer_node_id: NodeId,
        tokens: TokenStore,
        websocket_url: impl Into<String>,
    ) -> Self {
        Self {
            node_id,
            peer_node_id,
            tokens,
            websocket_url: websocket_url.into(),
            // The `v`-prefixed spelling, because `S2C §Versioning of JSON Schema
            // files` requires it over S2 Connect. Matching is canonical, so a peer that
            // offers the bare form is still understood (E25).
            s2_message_versions: alloc::vec![
                WireProfile::V1_0_0.connect_version(),
                WireProfile::V0_0_2Beta.connect_version(),
            ],
            protocols: alloc::vec![CommunicationProtocol::WebSocket],
            connection: None,
        }
    }

    /// Offer a different set of S2 JSON message versions, newest first.
    #[must_use]
    pub fn with_s2_message_versions(mut self, versions: Vec<ProtocolVersion>) -> Self {
        self.s2_message_versions = versions;
        self
    }

    /// Whether a bearer presented on either endpoint is one we would accept.
    ///
    /// A `false` is a `401`.
    #[must_use]
    pub fn authenticates(&self, bearer: &AccessToken) -> bool {
        self.tokens.accepts(bearer)
    }

    /// Handle `initiateSession`.
    ///
    /// `access_entropy` is a parameter for the same reason it is in
    /// [`pairing`](super::pairing): a test needs it pinned, and this layer has no business
    /// knowing where a system keeps its randomness.
    ///
    /// # Errors
    ///
    /// [`ConnectError::NotPaired`] is a `401`; a [`SessionRefused`] carried in
    /// [`ConnectError::SessionRefused`] is a `400` with that body.
    pub fn initiate_session(
        &mut self,
        bearer: &AccessToken,
        request: &SessionRequest,
        access_entropy: &[u8],
        now: Timestamp,
    ) -> Result<SessionGrant, ConnectError> {
        if !self.tokens.accepts(bearer) {
            return Err(ConnectError::NotPaired);
        }
        if request.server_node_id != self.node_id || request.client_node_id != self.peer_node_id {
            return Err(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::NoLongerPaired,
                additional_info: None,
            }));
        }
        let protocol = request
            .supported_communication_protocols
            .iter()
            .find(|p| self.protocols.contains(p))
            .copied()
            .ok_or(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::IncompatibleCommunicationProtocols,
                additional_info: None,
            }))?;
        // Newest first in the client's list; we take the first we also speak.
        let version = request
            .supported_s2_message_versions
            .iter()
            .find(|v| self.s2_message_versions.iter().any(|ours| ours.matches(v)))
            .cloned()
            .ok_or(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::IncompatibleS2MessageVersions,
                additional_info: None,
            }))?;

        let replacement = AccessToken::from_entropy(access_entropy)?;
        // Issued alongside, not instead of: the token the client just used stays valid
        // until it proves it has the new one. `add` records *when*, which is what makes
        // it a pending grant rather than simply another accepted token.
        self.tokens.add(replacement.clone(), now);
        Ok(SessionGrant {
            selected_communication_protocol: protocol,
            selected_s2_message_version: version,
            access_token: replacement,
            server_node_description: None,
            server_endpoint_description: None,
        })
    }

    /// Handle `confirmAccessToken`, retiring every older token.
    ///
    /// The bearer *is* the confirmation: a client that can present the new token has
    /// stored it, so there is nothing else to check and nothing to take on trust.
    pub fn confirm_access_token(
        &mut self,
        bearer: &AccessToken,
        communication_entropy: &[u8],
        now: Timestamp,
    ) -> Result<CommunicationDetails, ConnectError> {
        // The bearer must be a *pending grant*, not merely a token the store accepts:
        // presenting the replacement is the proof it was stored, and that proof is the
        // whole reason retiring the others is safe. The store is where that fact lives, so
        // it survives a restart between the two requests.
        if !self.tokens.is_pending(bearer) {
            return Err(ConnectError::NotPaired);
        }
        self.tokens.confirm(bearer, now)?;
        let token = CommunicationToken::from_entropy(communication_entropy)?;
        // Remembered, because a server that hands out a bearer it does not keep has
        // authorised nothing: the WebSocket upgrade later has nothing to check the
        // `Authorization` header against. One token at a time — a second confirmation
        // supersedes the first, exactly as it supersedes the access token.
        self.connection = Some(IssuedConnection {
            token: token.clone(),
            issued: now,
        });
        Ok(CommunicationDetails::WebSocket {
            websocket_url: self.websocket_url.clone(),
            websocket_token: token,
        })
    }

    /// Handle `unpair`.
    pub fn unpair(
        &mut self,
        bearer: &AccessToken,
        request: &UnpairRequest,
    ) -> Result<(), ConnectError> {
        if !self.tokens.accepts(bearer)
            || request.server_node_id != self.node_id
            || request.client_node_id != self.peer_node_id
        {
            return Err(ConnectError::NotPaired);
        }
        self.tokens.clear();
        self.connection = None;
        Ok(())
    }

    /// Whether a bearer presented on a WebSocket handshake is the one this server issued,
    /// and still inside its thirty seconds.
    ///
    /// Takes `&mut self` and consumes the token on success, because
    /// `s2-connect-common.yml/CommunicationToken` says it is "valid for a single
    /// connection": a second upgrade with the same bearer is refused rather than
    /// silently granted a parallel session.
    pub fn accepts_connection(&mut self, presented: &str, now: Timestamp) -> bool {
        let Some(connection) = &self.connection else {
            return false;
        };
        if now.saturating_duration_since(connection.issued) >= COMMUNICATION_TOKEN_LIFETIME {
            self.connection = None;
            return false;
        }
        if !connection.token.verify_str(presented) {
            return false;
        }
        self.connection = None;
        true
    }

    /// The tokens, to persist.
    #[must_use]
    pub fn tokens(&self) -> &TokenStore {
        &self.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_unix(1_700_000_000 + secs, 0)
    }

    #[test]
    fn a_grant_nobody_confirmed_never_evicts_the_token_that_works() {
        // Four `initiateSession` calls whose replies never reached the client each add a
        // pending grant. With a four-deep list and no expiry, the fourth pushes the token
        // that is actually in force off the end — and the pairing is dead while both
        // parties still believe in it. A grant past `PENDING_TOKEN_LIFETIME` can never be
        // confirmed, so it is a slot holding nothing and goes first.
        let live = AccessToken::from_entropy(&[9u8; 32]).expect("32 bytes");
        let mut store = TokenStore::new(live.clone());
        for i in 0..6u8 {
            let granted = AccessToken::from_entropy(&[i; 32]).expect("32 bytes");
            // Each attempt a minute after the last: every earlier grant is long dead.
            store.add(granted, t(i64::from(i) * 60));
        }
        assert!(
            store.accepts(&live),
            "the token the client is still holding must keep working"
        );
        assert_eq!(store.len(), 2, "one live token and the newest grant");

        // Grants that are still inside their window are *not* dropped: two initiations a
        // second apart are a client retrying, not a client leaking.
        let live = AccessToken::from_entropy(&[9u8; 32]).expect("32 bytes");
        let mut store = TokenStore::new(live.clone());
        for i in 0..3u8 {
            let granted = AccessToken::from_entropy(&[i; 32]).expect("32 bytes");
            store.add(granted, t(i64::from(i)));
        }
        assert_eq!(store.len(), 4);
        assert!(store.accepts(&live));
    }

    fn ids() -> (NodeId, NodeId) {
        (
            NodeId::parse("rm-1").expect("a legal id"),
            NodeId::parse("cem-1").expect("a legal id"),
        )
    }

    fn pair() -> (SessionInitClient, SessionInitServer, AccessToken) {
        let (rm, cem) = ids();
        let initial = AccessToken::from_entropy(&[1u8; 32]).expect("32 bytes");
        let client = SessionInitClient::new(rm, cem, TokenStore::new(initial.clone()));
        let server = SessionInitServer::new(
            cem,
            rm,
            TokenStore::new(initial.clone()),
            "wss://cem.example.com/s2",
        );
        (client, server, initial)
    }

    /// One whole initiation, both sides, with every step at `at`.
    fn run_at(
        client: &mut SessionInitClient,
        server: &mut SessionInitServer,
        new_access: u8,
        comm: u8,
        at: Timestamp,
    ) -> SessionCredentials {
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[new_access; 32], at)
            .unwrap();
        client.accept(grant, at).unwrap();
        let confirm = client.confirm().unwrap();
        let details = server
            .confirm_access_token(&confirm, &[comm; 32], at)
            .unwrap();
        client.connected(details, at).unwrap()
    }

    /// One whole initiation at the epoch of this module's clock.
    fn run(
        client: &mut SessionInitClient,
        server: &mut SessionInitServer,
        new_access: u8,
        comm: u8,
    ) -> SessionCredentials {
        run_at(client, server, new_access, comm, t(0))
    }

    #[test]
    fn a_session_is_initiated_and_the_token_rotates() {
        let (mut client, mut server, initial) = pair();
        let creds = run(&mut client, &mut server, 2, 3);

        assert_eq!(creds.websocket_url, "wss://cem.example.com/s2");
        assert_eq!(creds.profile, Some(WireProfile::V1_0_0));
        assert!(!creds.is_expired(t(29)));

        // The old token is gone on both sides; the new one works on both.
        assert!(!server.tokens().accepts(&initial));
        assert_eq!(client.tokens().len(), 1);
        assert!(server.tokens().accepts(client.tokens().current().unwrap()));
    }

    #[test]
    fn the_confirmation_is_authenticated_with_the_new_token() {
        // That is the whole trick: presenting it *is* the proof it was stored.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        let replacement = grant.access_token.clone();
        client.accept(grant, t(0)).unwrap();

        // The *old* token does not confirm anything.
        assert!(matches!(
            server.confirm_access_token(&initial, &[3u8; 32], t(0)),
            Err(ConnectError::NotPaired)
        ));
        // The new one does.
        assert!(
            server
                .confirm_access_token(&replacement, &[3u8; 32], t(0))
                .is_ok()
        );
    }

    #[test]
    fn a_lost_grant_does_not_strand_the_pairing() {
        // The reply to initiateSession never arrives: the server has issued a replacement
        // the client has never seen, and the client still holds the old one.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let _lost = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();

        // The client retries with what it has, and the server still accepts it.
        client.restart();
        assert!(server.tokens().accepts(&initial));
        let creds = run(&mut client, &mut server, 4, 5);
        assert!(!creds.websocket_url.is_empty());
        assert!(server.tokens().accepts(client.tokens().current().unwrap()));
    }

    #[test]
    fn a_lost_confirmation_does_not_strand_the_pairing_either() {
        // The client stored the new token and then died before confirming.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();

        // Both tokens still open the door — the point of the phase.
        assert!(server.tokens().accepts(&initial));
        assert!(server.tokens().accepts(client.tokens().current().unwrap()));
        assert_eq!(client.tokens().len(), 2);

        client.restart();
        let creds = run(&mut client, &mut server, 6, 7);
        assert!(!creds.websocket_url.is_empty());
        assert_eq!(
            client.tokens().len(),
            1,
            "and the list collapses on success"
        );
    }

    #[test]
    fn a_client_works_down_its_list_after_a_401() {
        // Two failed rounds have left three tokens; only the newest is live.
        let (mut client, mut server, _) = pair();
        for entropy in [2u8, 3u8] {
            let (bearer, request) = client.initiate().unwrap();
            let grant = server
                .initiate_session(&bearer, &request, &[entropy; 32], t(0))
                .unwrap();
            client.accept(grant, t(0)).unwrap();
            client.restart();
        }
        assert_eq!(client.tokens().len(), 3);

        // A server that has forgotten everything refuses each in turn, and the client
        // concludes it is unpaired rather than retrying for ever.
        let (rm, cem) = ids();
        let mut amnesiac = SessionInitServer::new(cem, rm, TokenStore::default(), "wss://x/");
        for _ in 0..3 {
            let (bearer, request) = client.initiate().unwrap();
            assert!(matches!(
                amnesiac.initiate_session(&bearer, &request, &[9u8; 32], t(0)),
                Err(ConnectError::NotPaired)
            ));
            if let Err(e) = client.unauthorized() {
                assert_eq!(e, ConnectError::NotPaired);
                return;
            }
        }
        panic!("the client should have given up after exhausting its tokens");
    }

    #[test]
    fn a_token_the_server_never_issued_is_refused() {
        let (mut client, mut server, _) = pair();
        let (_, request) = client.initiate().unwrap();
        let forged = AccessToken::from_entropy(&[0xFFu8; 32]).unwrap();
        assert!(matches!(
            server.initiate_session(&forged, &request, &[2u8; 32], t(0)),
            Err(ConnectError::NotPaired)
        ));
    }

    #[test]
    fn a_request_for_someone_elses_pairing_is_refused() {
        let (mut client, mut server, _) = pair();
        let (bearer, mut request) = client.initiate().unwrap();
        request.server_node_id = NodeId::parse("cem-2").unwrap();
        assert!(matches!(
            server.initiate_session(&bearer, &request, &[2u8; 32], t(0)),
            Err(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::NoLongerPaired,
                ..
            }))
        ));
    }

    #[test]
    fn a_peer_that_spells_the_version_the_other_way_still_connects() {
        // Erratum E25: `S2C` requires `v1.0.0`, every deployed S2 JSON handshake writes
        // `1.0.0`, and `s2energy-connection`'s examples write `v1`. Exact matching on
        // both sides means two conforming implementations cannot agree on a version they
        // both speak — which is a pairing that fails for a reason no log would explain.
        for spelling in ["v1.0.0", "1.0.0"] {
            let (client, mut server, _) = pair();
            let mut client =
                client.with_s2_message_versions(alloc::vec![ProtocolVersion::new(spelling)]);
            let creds = run(&mut client, &mut server, 2, 3);
            assert_eq!(
                creds.profile,
                Some(WireProfile::V1_0_0),
                "a server offering the other spelling must still resolve {spelling}"
            );
        }

        // And the same in the other direction: a server that offers only the bare form
        // answers a client that offered only the prefixed one.
        let (mut client, server, _) = pair();
        let mut server =
            server.with_s2_message_versions(alloc::vec![ProtocolVersion::new("1.0.0")]);
        let creds = run(&mut client, &mut server, 2, 3);
        assert_eq!(creds.profile, Some(WireProfile::V1_0_0));
    }

    #[test]
    fn a_version_neither_side_speaks_is_refused_with_the_named_reason() {
        let (rm, cem) = ids();
        let initial = AccessToken::from_entropy(&[1u8; 32]).unwrap();
        let mut client = SessionInitClient::new(rm, cem, TokenStore::new(initial.clone()))
            .with_s2_message_versions(alloc::vec![crate::types::ProtocolVersion::new("9.9.9")]);
        let mut server = SessionInitServer::new(cem, rm, TokenStore::new(initial), "wss://x/");
        let (bearer, request) = client.initiate().unwrap();
        assert!(matches!(
            server.initiate_session(&bearer, &request, &[2u8; 32], t(0)),
            Err(ConnectError::SessionRefused(SessionRefused {
                error_message: SessionErrorMessage::IncompatibleS2MessageVersions,
                ..
            }))
        ));
    }

    #[test]
    fn a_communication_token_is_good_for_thirty_seconds_and_not_a_moment_more() {
        let (mut client, mut server, _) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();
        let confirm = client.confirm().unwrap();
        let details = server
            .confirm_access_token(&confirm, &[3u8; 32], t(0))
            .unwrap();
        let CommunicationDetails::WebSocket {
            ref websocket_token,
            ..
        } = details;
        let presented = websocket_token.as_str().to_string();
        let creds = client.connected(details, t(0)).unwrap();

        assert!(creds.is_expired(t(30)));
        assert_eq!(creds.expires(), t(30));

        // The right timing with the wrong token is refused, and refusing it does not
        // spend the one the server is holding.
        assert!(!server.accepts_connection("something else", t(1)));
        // The right token inside the window is accepted...
        assert!(server.accepts_connection(&presented, t(29)));
        // ...exactly once: a communication token is "valid for a single connection".
        assert!(!server.accepts_connection(&presented, t(29)));
    }

    #[test]
    fn a_communication_token_presented_too_late_is_refused() {
        let (mut client, mut server, _) = pair();
        let creds = run(&mut client, &mut server, 2, 3);
        let presented = creds.websocket_token.as_str().to_string();
        assert!(!server.accepts_connection(&presented, t(30)));
        // And the expired entry is gone, so it cannot come back inside the window.
        assert!(!server.accepts_connection(&presented, t(1)));
    }

    #[test]
    fn a_server_restarted_between_the_two_requests_still_honours_the_grant() {
        // The pending grant lives in the `TokenStore`, which is the thing a host
        // persists — not in a field of the state machine. A server that kept it in memory
        // would refuse the confirmation of a grant it had itself issued a second earlier,
        // and the client would be left holding a token nobody activated.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();
        let confirm = client.confirm().unwrap();

        // The process dies here. Everything it persisted comes back; everything else
        // does not.
        let persisted = server.tokens().clone();
        let (rm, cem) = ids();
        let mut server = SessionInitServer::new(cem, rm, persisted, "wss://cem.example.com/s2");

        let details = server
            .confirm_access_token(&confirm, &[3u8; 32], t(1))
            .expect("a restart must not lose the grant");
        let creds = client.connected(details, t(1)).unwrap();
        assert!(
            !server.tokens().accepts(&initial),
            "the old token is retired"
        );
        assert!(server.tokens().accepts(client.tokens().current().unwrap()));
        assert!(!creds.is_expired(t(2)));
    }

    #[test]
    fn only_a_pending_grant_confirms_anything() {
        // Every token the store accepts is not a token that confirms: presenting the
        // *replacement* is the proof it was stored, which is the only reason retiring the
        // others is safe. A client that presents the token it already had proves nothing.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();

        assert!(
            server.tokens().accepts(&initial),
            "it is still a valid bearer"
        );
        assert!(
            matches!(
                server.confirm_access_token(&initial, &[3u8; 32], t(0)),
                Err(ConnectError::NotPaired)
            ),
            "but it is not the grant"
        );
        // And the real one still works afterwards: a wrong confirmation must not burn it.
        let confirm = client.confirm().unwrap();
        assert!(
            server
                .confirm_access_token(&confirm, &[3u8; 32], t(0))
                .is_ok()
        );
    }

    #[test]
    fn a_grant_confirmed_too_late_leaves_the_old_token_in_force() {
        // `S2C §Session initiation`: the server activates the replacement only if the
        // confirmation arrives "not more than 15 seconds" later. The point of refusing a
        // late one is what it does *not* do — it must not retire the token the client is
        // still holding, or a slow network unpairs a device.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();
        let confirm = client.confirm().unwrap();

        let too_late = server.confirm_access_token(&confirm, &[3u8; 32], t(16));
        assert!(
            matches!(
                too_late,
                Err(ConnectError::Expired {
                    what: "access token confirmation",
                    ..
                })
            ),
            "{too_late:?}"
        );
        assert!(
            server.tokens().accepts(&initial),
            "the old token still works"
        );

        // Fourteen seconds later is inside the window, and then the old one is gone.
        let (mut client, mut server, initial) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        client.accept(grant, t(0)).unwrap();
        let confirm = client.confirm().unwrap();
        let details = server
            .confirm_access_token(&confirm, &[3u8; 32], t(14))
            .expect("fourteen seconds is inside fifteen");
        let creds = client.connected(details, t(14)).unwrap();
        assert!(!server.tokens().accepts(&initial));
        assert_eq!(creds.profile, Some(WireProfile::V1_0_0));
    }

    #[test]
    fn the_phases_cannot_be_taken_out_of_order() {
        let (mut client, _, _) = pair();
        assert!(matches!(
            client.confirm(),
            Err(ConnectError::OutOfOrder { .. })
        ));
        assert!(client.initiate().is_err());
        client.restart();
        assert!(client.initiate().is_ok());
    }

    #[test]
    fn a_completed_session_can_be_followed_by_another() {
        // A communication token authorises one connection and lives thirty seconds, so
        // reconnecting is the normal case, not an exception.
        let (mut client, mut server, _) = pair();
        run(&mut client, &mut server, 2, 3);
        run(&mut client, &mut server, 4, 5);
        run(&mut client, &mut server, 6, 7);
        // Each round retires the last token, so the list never grows.
        assert_eq!(client.tokens().len(), 1);
        assert!(server.tokens().accepts(client.tokens().current().unwrap()));
    }

    #[test]
    fn unpairing_forgets_the_pairing_on_both_sides() {
        let (mut client, mut server, _) = pair();
        run(&mut client, &mut server, 2, 3);
        let token = client.tokens().current().unwrap().clone();
        let request = client.unpaired();
        server.unpair(&token, &request).unwrap();
        assert!(server.tokens().is_empty());
        assert!(client.tokens().is_empty());
        // And nothing works afterwards.
        assert!(!server.authenticates(&token));
    }

    #[test]
    fn the_bodies_are_the_shape_the_openapi_defines() {
        let (mut client, mut server, _) = pair();
        let (bearer, request) = client.initiate().unwrap();
        let json = serde_json::to_value(&request).unwrap();
        for key in [
            "clientNodeId",
            "serverNodeId",
            "supportedS2MessageVersions",
            "supportedCommunicationProtocols",
        ] {
            assert!(json.get(key).is_some(), "missing required property {key}");
        }
        assert!(json.get("clientNodeDescription").is_none());
        assert_eq!(json["supportedCommunicationProtocols"][0], "WebSocket");

        let grant = server
            .initiate_session(&bearer, &request, &[2u8; 32], t(0))
            .unwrap();
        let json = serde_json::to_value(&grant).unwrap();
        for key in [
            "selectedCommunicationProtocol",
            "selectedS2MessageVersion",
            "accessToken",
        ] {
            assert!(json.get(key).is_some(), "missing required property {key}");
        }

        // The discriminated union the schema calls WebSocketCommunicationDetails.
        client.accept(grant, t(0)).unwrap();
        let confirm = client.confirm().unwrap();
        let details = server
            .confirm_access_token(&confirm, &[3u8; 32], t(0))
            .unwrap();
        let json = serde_json::to_value(&details).unwrap();
        assert_eq!(json["communicationProtocol"], "WebSocket");
        assert!(json.get("websocketUrl").is_some());
        assert!(json.get("websocketToken").is_some());
    }
}
