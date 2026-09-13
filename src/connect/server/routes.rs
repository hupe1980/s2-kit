//! The router: URLs in, status codes out.
//!
//! The mapping from a protocol outcome to a status code is the whole content of this
//! file, and it is not mechanical — see the table in the [module documentation](super).

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::connect::proto::{
    AccessToken, ConnectError, ConnectionDetailsPost, ConnectionDetailsRequest, FinalizePairing,
    NodeId, PairingAttemptId, PairingRequest,
};
use crate::types::Timestamp;

use super::longpoll::{LongPoll, WaitingNode};
use super::pairing::PairingService;
use super::session::SessionService;
use super::subnet::LocalSubnets;

/// Everything the routes need.
///
/// Cloneable and cheap: an `Arc` of each service.
#[derive(Clone)]
pub struct Endpoint {
    pairing: Arc<PairingService>,
    session: Arc<SessionService>,
    /// Which node each live pairing attempt is for, keyed by its bearer, and when that
    /// attempt stops being live.
    ///
    /// The attempt id authenticates steps 6–8 and the node is what routes them; keeping
    /// the map here rather than in the service leaves the service free of HTTP concerns.
    ///
    /// The deadline is not decoration. `requestPairing` needs no bearer, so anyone who
    /// can reach the endpoint can add an entry, and a client that opens an attempt and
    /// then vanishes never reaches `finalizePairing` to remove it. Without expiry this
    /// map is unbounded growth driven by unauthenticated input — slow, because the
    /// per-node rate limit caps it at one entry per second per node, but a device that
    /// runs for years does not get to leak slowly.
    attempts: Arc<std::sync::Mutex<Vec<(PairingAttemptId, Attempt)>>>,
    /// What this endpoint has to tell nodes that are long-polling it.
    long_poll: Arc<LongPoll>,
    clock: fn() -> Timestamp,
    /// The largest WebSocket frame this endpoint will buffer.
    ///
    /// Here rather than on the route because it is the *endpoint's* budget: the same
    /// number the codec and the session engine use, applied where it bounds memory rather
    /// than where it bounds parsing.
    max_message_bytes: usize,
    /// What to do when a client says the user has started pairing on their side.
    on_prepare: Option<Arc<dyn Fn(PrepareSignal) + Send + Sync>>,
}

/// The "a user is about to pair with you" signal, and its cancellation.
///
/// `S2C §preparePairing`: "It is up to the server implementation to decide what to do with
/// this signal, but it can be used to display a pop-up with the pairing token in its UI to
/// improve the user experience." A device with a screen shows the code at the moment
/// somebody is looking for it, rather than making them go and find it.
///
/// Best effort by design: "When a preparePairing is called, it is not guaranteed that a
/// call to pairingRequest or cancelPreparePairing will follow." Anything a handler starts
/// needs its own timeout.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum PrepareSignal {
    /// The user has started pairing on the client and wants this node's code.
    Prepare(crate::connect::proto::PreparePairing),
    /// They changed their mind.
    Cancel(crate::connect::proto::CancelPreparePairing),
}

/// A pairing attempt in flight, from the router's point of view.
#[derive(Debug, Clone, Copy)]
struct Attempt {
    node: NodeId,
    /// After this, the state machine would refuse the attempt anyway.
    expires: Timestamp,
}

impl core::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Endpoint")
            .field("pairing", &self.pairing)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Endpoint {
    /// An endpoint serving both APIs.
    #[must_use]
    pub fn new(pairing: Arc<PairingService>, session: Arc<SessionService>) -> Self {
        Self {
            pairing,
            session,
            attempts: Arc::new(std::sync::Mutex::new(Vec::new())),
            long_poll: Arc::new(LongPoll::default()),
            clock: Timestamp::now,
            max_message_bytes: crate::codec::DecodeOptions::DEFAULT_MAX_BYTES,
            on_prepare: None,
        }
    }

    /// Buffer no WebSocket frame larger than this.
    ///
    /// Defaults to
    /// [`DecodeOptions::DEFAULT_MAX_BYTES`](crate::codec::DecodeOptions::DEFAULT_MAX_BYTES),
    /// the mebibyte the codec and the session engines already use. Raise it only together
    /// with the session's own `max_message_bytes`: a frame larger than the codec will
    /// parse is a frame that is buffered and then refused.
    #[must_use]
    pub const fn with_max_message_bytes(mut self, bytes: usize) -> Self {
        self.max_message_bytes = bytes;
        self
    }

    /// The largest WebSocket frame this endpoint will buffer.
    #[must_use]
    pub const fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }

    /// React to a client announcing that its user has started pairing.
    ///
    /// Typically: show the pairing code on a display, or light an LED. The handler runs
    /// inside the request, so it must not block — and it must not be trusted, because the
    /// signal is unauthenticated and only reaches you at all through the same-subnet
    /// check in [`lan_router`].
    #[must_use]
    pub fn on_prepare_pairing(
        mut self,
        handler: impl Fn(PrepareSignal) + Send + Sync + 'static,
    ) -> Self {
        self.on_prepare = Some(Arc::new(handler));
        self
    }

    /// Use a different clock — a virtual one, in a test.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> Timestamp) -> Self {
        self.clock = clock;
        self
    }

    /// The session service, for a host that also terminates the WebSocket.
    #[must_use]
    pub fn session(&self) -> &Arc<SessionService> {
        &self.session
    }

    fn remember(&self, id: PairingAttemptId, node: NodeId, now: Timestamp) {
        let Ok(mut attempts) = self.attempts.lock() else {
            return;
        };
        // Sweep on insert rather than on a timer: the only thing that grows this list is
        // the operation that also cleans it, so it cannot outrun its own collection.
        attempts.retain(|(_, attempt)| attempt.expires > now);
        attempts.push((
            id,
            Attempt {
                node,
                expires: now
                    .checked_add(crate::connect::proto::PAIRING_BUDGET)
                    .unwrap_or(now),
            },
        ));
    }

    /// Which node a bearer's attempt is for, if it is still live.
    ///
    /// A list scanned with a constant-time comparison rather than a map keyed by the
    /// bearer: the attempt id *is* a secret — it authenticates steps 6 to 8 — and how long
    /// a tree takes to find a key is a function of the key. The list holds at most one
    /// live entry per node, because the rate limit allows one attempt per node per second
    /// and each expires after fifteen, so scanning it costs nothing worth saving.
    ///
    /// An expired entry is treated as absent, which the caller turns into `401` — and
    /// `S2C-OAS` says a `401` here lets the client restart the pairing, which is exactly
    /// the right thing to do with an attempt whose fifteen seconds have gone.
    fn attempt_node(&self, bearer: &str, now: Timestamp) -> Option<NodeId> {
        let attempts = self.attempts.lock().ok()?;
        // Every entry is examined, so how long this takes does not say which matched.
        let mut found = None;
        for (id, attempt) in attempts.iter() {
            if id.verify(bearer) && attempt.expires > now {
                found = Some(attempt.node);
            }
        }
        found
    }

    fn forget(&self, bearer: &str) {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.retain(|(id, _)| !id.verify(bearer));
        }
    }

    /// The most long-polling nodes one endpoint will remember.
    ///
    /// `waitForPairing` carries no bearer and its body is a list of identifiers the caller
    /// chose, so the table it fills is bounded. A LAN endpoint talks to tens of devices.
    pub const MAX_WAITING_NODES: usize = LongPoll::MAX_WAITING;

    /// The most nodes a single `waitForPairing` may ask about.
    pub const MAX_NODES_PER_POLL: usize = LongPoll::MAX_PER_POLL;

    /// Tell a long-polling node to do something the next time it asks.
    ///
    /// This is the other half of [`lan_router`]'s `waitForPairing`: a Resource Manager
    /// with no listening socket cannot be dialled, so when a person presses the pairing
    /// button *here* the instruction waits until that node next checks in. A request that
    /// is already held open is woken at once, so "waits" is usually microseconds.
    pub fn instruct_waiting_node(&self, client: NodeId, action: crate::connect::proto::WaitAction) {
        self.long_poll.instruct(client, action);
    }

    /// Every node that is long-polling this endpoint, and what it has said about itself.
    ///
    /// What a user interface lists when somebody asks "what is out there to pair with?" —
    /// the counterpart of `GET /v1/nodes` for clients that have no server to browse.
    #[must_use]
    pub fn waiting_nodes(&self) -> Vec<(NodeId, WaitingNode)> {
        self.long_poll.waiting()
    }

    /// This endpoint's clock, which the WebSocket route reads too.
    pub(super) fn now(&self) -> Timestamp {
        (self.clock)()
    }

    /// How many attempts the router is tracking. For a test, and for a health endpoint.
    #[must_use]
    pub fn live_attempts(&self) -> usize {
        self.attempts.lock().map_or(0, |attempts| attempts.len())
    }
}

/// The router for an S2 Connect endpoint, mounted at `/v1`.
///
/// `S2C §Formal specification and versioning` puts every operation under a version
/// segment, and the OpenAPI's `servers` entry is `/v1`. Mounting it anywhere else makes
/// an endpoint no client can talk to.
pub fn router(endpoint: Endpoint) -> axum::Router {
    axum::Router::new()
        .nest("/v1", authenticated_routes(endpoint))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// The largest request body any S2 Connect operation will read.
///
/// `requestPairing` needs no bearer, so the number is not a tuning knob — it is the point
/// at which an unauthenticated caller stops being able to make this process allocate. The
/// biggest legitimate body is a `requestPairing` carrying two node descriptions, two
/// endpoint descriptions and a Base64 challenge: a few kilobytes. Sixty-four is generous
/// enough that no conforming peer will ever meet it and small enough that meeting it
/// costs nothing.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// The router for a **LAN** endpoint: everything above, plus the operations only LAN
/// endpoints implement, each behind the same-subnet check they require.
///
/// This is a separate function rather than a flag because the unauthenticated operations
/// need to be impossible to expose by accident. `GET /v1/nodes` tells whoever asks the
/// brand, model and role of every flexible device in the building, and
/// `POST /v1/preparePairing` makes a device display its pairing code — neither carries a
/// bearer, a challenge or a signature. The subnet *is* the access control, so it is not
/// optional and cannot be forgotten: you cannot mount these routes without supplying
/// [`LocalSubnets`].
///
/// A WAN endpoint uses [`router`] instead. The absent routes then answer `404`, which is
/// what `S2C §LAN-LAN only interactions` recommends: "WAN endpoints **cannot** implement
/// these operations. It is **recommended** that WAN endpoints respond with status code
/// 404."
///
/// Requires the server to have been started with
/// `into_make_service_with_connect_info::<SocketAddr>()`; without a peer address the
/// subnet check fails closed and every LAN-only request is refused.
pub fn lan_router(endpoint: Endpoint, subnets: LocalSubnets) -> axum::Router {
    let lan_only = axum::Router::new()
        .route("/endpoint", get(get_endpoint))
        .route("/nodes", get(get_nodes))
        .route("/preparePairing", post(prepare_pairing))
        .route("/cancelPreparePairing", post(cancel_prepare_pairing))
        .route("/waitForPairing", post(wait_for_pairing))
        .with_state(endpoint.clone())
        .layer(axum::middleware::from_fn_with_state(
            subnets,
            super::subnet::same_subnet,
        ));

    axum::Router::new()
        .nest("/v1", authenticated_routes(endpoint).merge(lan_only))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// The operations every endpoint implements, LAN or WAN.
///
/// Each is authenticated: `requestPairing` by the pairing token it proves knowledge of,
/// the rest by a bearer.
fn authenticated_routes(endpoint: Endpoint) -> axum::Router {
    axum::Router::new()
        .route("/requestPairing", post(request_pairing))
        .route(
            "/requestConnectionDetails",
            post(request_connection_details),
        )
        .route("/postConnectionDetails", post(post_connection_details))
        .route("/finalizePairing", post(finalize_pairing))
        .route("/initiateSession", post(initiate_session))
        .route("/confirmAccessToken", post(confirm_access_token))
        .route("/unpair", post(unpair))
        .with_state(endpoint)
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

async fn request_pairing(
    State(endpoint): State<Endpoint>,
    Json(request): Json<PairingRequest>,
) -> Response {
    let mut nonce = [0u8; 32];
    let mut attempt = [0u8; 24];
    if fill_random(&mut nonce).is_err() || fill_random(&mut attempt).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let Ok(attempt_id) = PairingAttemptId::from_entropy(&attempt) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let bearer = attempt_id.clone();
    let now = (endpoint.clock)();

    match endpoint
        .pairing
        .request_pairing(&request, &nonce, attempt_id, now)
    {
        Ok((node, send_after, accepted)) => {
            // The mandatory delay, honoured by actually waiting. A server that computes
            // `send_after` and answers anyway has implemented a comment, not a rule.
            let wait = send_after.saturating_duration_since((endpoint.clock)());
            if !wait.is_zero() {
                tokio::time::sleep(core::time::Duration::from(wait)).await;
            }
            endpoint.remember(bearer, node, now);
            crate::trace::event!(info, node = %node, "answered a pairing request");
            (StatusCode::OK, Json(accepted)).into_response()
        }
        Err(e) => {
            crate::trace::event!(warn, error = %e, "refused a pairing request");
            pairing_error(e)
        }
    }
}

async fn request_connection_details(
    State(endpoint): State<Endpoint>,
    bearer: Bearer,
    Json(request): Json<ConnectionDetailsRequest>,
) -> Response {
    let now = (endpoint.clock)();
    let Some(node) = endpoint.attempt_node(&bearer.0, now) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match endpoint
        .pairing
        .request_connection_details(
            node,
            &bearer.0,
            &request.server_hmac_challenge_response,
            now,
        )
        .await
    {
        Ok(details) => (StatusCode::OK, Json(details)).into_response(),
        Err(e) => {
            if matches!(e, ConnectError::ChallengeFailed) {
                endpoint.forget(&bearer.0);
            }
            pairing_error(e)
        }
    }
}

async fn post_connection_details(
    State(endpoint): State<Endpoint>,
    bearer: Bearer,
    Json(request): Json<ConnectionDetailsPost>,
) -> Response {
    let now = (endpoint.clock)();
    let Some(node) = endpoint.attempt_node(&bearer.0, now) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match endpoint
        .pairing
        .post_connection_details(
            node,
            &bearer.0,
            &request.server_hmac_challenge_response,
            request.connection_details,
            now,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            if matches!(e, ConnectError::ChallengeFailed) {
                endpoint.forget(&bearer.0);
            }
            pairing_error(e)
        }
    }
}

async fn finalize_pairing(
    State(endpoint): State<Endpoint>,
    bearer: Bearer,
    Json(body): Json<FinalizePairing>,
) -> Response {
    let now = (endpoint.clock)();
    let Some(node) = endpoint.attempt_node(&bearer.0, now) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let result = endpoint
        .pairing
        .finalize(node, &bearer.0, body.success, now)
        .await;
    // The attempt is over either way: "Once the pairing process is finished (with or
    // without success) the identifier can be discarded."
    endpoint.forget(&bearer.0);
    match result {
        Ok(()) => {
            crate::trace::event!(
                info,
                node = %node,
                success = body.success,
                "pairing finalized"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            crate::trace::event!(warn, error = %e, "finalization refused");
            pairing_error(e)
        }
    }
}

// ---------------------------------------------------------------------------
// LAN-only informational routes
// ---------------------------------------------------------------------------

async fn get_endpoint(State(endpoint): State<Endpoint>) -> Response {
    // The schema is `EndpointDescription` itself, with no envelope.
    (StatusCode::OK, Json(endpoint.pairing.endpoint().clone())).into_response()
}

async fn get_nodes(State(endpoint): State<Endpoint>) -> Response {
    // Likewise a bare array of `NodeDescription`. The alias each node is known by is
    // deliberately not in it (erratum E21).
    let nodes: Vec<crate::connect::proto::NodeDescription> = endpoint
        .pairing
        .store()
        .nodes()
        .into_iter()
        .map(|(node, _alias)| node)
        .collect();
    (StatusCode::OK, Json(nodes)).into_response()
}

async fn prepare_pairing(
    State(endpoint): State<Endpoint>,
    Json(body): Json<crate::connect::proto::PreparePairing>,
) -> Response {
    // "204: Notification received (also used when provided serverNodeId is not known)" —
    // so an unknown node is not an error, and telling the caller which node identifiers
    // exist would be a disclosure the operation does not owe them.
    if let Some(handler) = &endpoint.on_prepare {
        handler(PrepareSignal::Prepare(body));
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn cancel_prepare_pairing(
    State(endpoint): State<Endpoint>,
    Json(body): Json<crate::connect::proto::CancelPreparePairing>,
) -> Response {
    if let Some(handler) = &endpoint.on_prepare {
        handler(PrepareSignal::Cancel(body));
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /v1/waitForPairing` — held open until there is something to say.
///
/// Always `200`, with a list that may be empty: "nothing yet, ask again" is the normal
/// answer and must not look like a failure, or a client that is working perfectly spends
/// its life backing off.
async fn wait_for_pairing(
    State(endpoint): State<Endpoint>,
    Json(nodes): Json<Vec<crate::connect::proto::WaitForPairing>>,
) -> Response {
    let instructions = endpoint.long_poll.poll(&nodes).await;
    (StatusCode::OK, Json(instructions)).into_response()
}

// ---------------------------------------------------------------------------
// Session initiation
// ---------------------------------------------------------------------------

async fn initiate_session(
    State(endpoint): State<Endpoint>,
    bearer: Bearer,
    Json(request): Json<crate::connect::proto::SessionRequest>,
) -> Response {
    let mut entropy = [0u8; 32];
    if fill_random(&mut entropy).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let token = AccessToken::new(bearer.0);
    let now = (endpoint.clock)();
    match endpoint
        .session
        .initiate_session(&token, &request, &entropy, now)
    {
        Ok(grant) => (StatusCode::OK, Json(grant)).into_response(),
        Err(e) => session_error(e),
    }
}

async fn confirm_access_token(State(endpoint): State<Endpoint>, bearer: Bearer) -> Response {
    let mut entropy = [0u8; 32];
    if fill_random(&mut entropy).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let token = AccessToken::new(bearer.0);
    let now = (endpoint.clock)();
    match endpoint.session.confirm_access_token(&token, &entropy, now) {
        Ok((remote, details)) => {
            crate::trace::event!(info, node = %remote, "issued a communication token");
            (StatusCode::OK, Json(details)).into_response()
        }
        // A grant that was not confirmed inside `PENDING_TOKEN_LIFETIME` reads the same
        // way to the client as one that was never issued: start again. The token it
        // already holds is untouched, so starting again is all it has to do.
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn unpair(
    State(endpoint): State<Endpoint>,
    bearer: Bearer,
    Json(request): Json<crate::connect::proto::UnpairRequest>,
) -> Response {
    let token = AccessToken::new(bearer.0);
    match endpoint.session.unpair(&token, &request) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// The `Authorization: Bearer …` header, which every step after the first carries.
pub(super) struct Bearer(String);

impl Bearer {
    /// The token as presented. Compare it in constant time, never with `==`.
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Bearer {
    type Rejection = StatusCode;

    // The trait is async; reading a header is not.
    #[allow(clippy::unused_async_trait_impl)]
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            // RFC 9110 §11.4: "the scheme is a case-insensitive token". A client that
            // writes `bearer` is conforming, and refusing it is our bug, not theirs.
            .and_then(|value| {
                let (scheme, token) = value.split_once(' ')?;
                scheme.eq_ignore_ascii_case("Bearer").then_some(token)
            })
            .map(|token| Self(token.trim().to_string()))
            .filter(|bearer| !bearer.0.is_empty())
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

/// The mapping the specification's recovery logic depends on.
fn pairing_error(error: ConnectError) -> Response {
    match error {
        // "The server is temporarily not able to process the pairing request, try again
        // soon." Not a failure; a client that treats it as one gives up too early.
        ConnectError::RateLimited => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        // "Provided pairingAttemptId not accepted. The client may restart the pairing
        // process by calling /requestPairing again."
        ConnectError::NotPaired => StatusCode::UNAUTHORIZED.into_response(),
        // "The PairingAttempt has failed and cannot proceed." Distinct from 401 on
        // purpose: restarting is the only option, and the client must not retry the step.
        ConnectError::ChallengeFailed => StatusCode::FORBIDDEN.into_response(),
        ConnectError::Refused(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
        ConnectError::Expired { .. } | ConnectError::OutOfOrder { .. } => {
            StatusCode::BAD_REQUEST.into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

fn session_error(error: ConnectError) -> Response {
    match error {
        // "Unauthorized, combination of clientNodeId and accessToken not accepted."
        ConnectError::NotPaired => StatusCode::UNAUTHORIZED.into_response(),
        ConnectError::SessionRefused(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

fn fill_random(buffer: &mut [u8]) -> Result<(), rand::rngs::SysError> {
    use rand::TryRng as _;
    rand::rngs::SysRng.try_fill_bytes(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_unix(1_700_000_000 + secs, 0)
    }

    fn endpoint() -> Endpoint {
        let store: Arc<dyn super::super::store::Store> = super::super::store::Memory::new();
        Endpoint::new(
            Arc::new(PairingService::wan(
                store.clone(),
                crate::connect::proto::EndpointDescription::default(),
                "pairing.example.com",
            )),
            Arc::new(SessionService::new(store, "wss://example.com/s2")),
        )
    }

    #[test]
    fn an_attempt_stops_authenticating_when_its_budget_runs_out() {
        let endpoint = endpoint();
        let node = NodeId::parse("cem-1").expect("a legal id");
        endpoint.remember(PairingAttemptId::new("bearer-one"), node, at(0));

        assert_eq!(endpoint.attempt_node("bearer-one", at(0)), Some(node));
        assert_eq!(endpoint.attempt_node("bearer-one", at(14)), Some(node));
        // Fifteen seconds is the whole budget; after it the state machine would refuse
        // the attempt anyway, so the router stops routing to it.
        assert_eq!(endpoint.attempt_node("bearer-one", at(15)), None);
        assert_eq!(endpoint.attempt_node("bearer-one", at(16)), None);
        // And something nobody issued never authenticated in the first place.
        assert_eq!(endpoint.attempt_node("not-a-bearer", at(0)), None);
    }

    #[test]
    fn abandoned_attempts_are_swept_rather_than_accumulated() {
        // `requestPairing` carries no bearer, so anyone who can reach the endpoint can
        // add an entry, and a client that walks away never removes it. Unbounded growth
        // driven by unauthenticated input is not something a device running for years
        // can afford.
        let endpoint = endpoint();
        let node = NodeId::parse("cem-1").expect("a legal id");

        for i in 0..100 {
            endpoint.remember(
                PairingAttemptId::new(alloc::format!("abandoned-{i}")),
                node,
                at(i),
            );
        }
        // Each insert sweeps, so only the ones still inside their budget survive.
        assert!(
            endpoint.live_attempts() <= 16,
            "expected at most one budget's worth, got {}",
            endpoint.live_attempts()
        );

        // Long afterwards, one more request clears the lot.
        endpoint.remember(PairingAttemptId::new("fresh"), node, at(1_000));
        assert_eq!(endpoint.live_attempts(), 1);
        assert_eq!(
            endpoint.attempt_node("fresh", at(1_000)),
            Some(node),
            "and the new one is the survivor"
        );
    }

    #[test]
    fn finalizing_forgets_the_attempt_immediately() {
        // "Once the pairing process is finished (with or without success) the identifier
        // can be discarded" — not fifteen seconds later.
        let endpoint = endpoint();
        let node = NodeId::parse("cem-1").expect("a legal id");
        endpoint.remember(PairingAttemptId::new("bearer"), node, at(0));
        assert_eq!(endpoint.live_attempts(), 1);
        endpoint.forget("bearer");
        assert_eq!(endpoint.live_attempts(), 0);
        assert_eq!(endpoint.attempt_node("bearer", at(0)), None);
    }

    #[test]
    fn the_status_codes_are_the_ones_the_specification_distinguishes() {
        // These four are the difference between a client that recovers and one that does
        // not, and they are easy to collapse into a single 400.
        let code = |e: ConnectError| pairing_error(e).status();
        assert_eq!(
            code(ConnectError::RateLimited),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(code(ConnectError::NotPaired), StatusCode::UNAUTHORIZED);
        assert_eq!(code(ConnectError::ChallengeFailed), StatusCode::FORBIDDEN);
        assert_eq!(
            code(ConnectError::Refused(
                crate::connect::proto::PairingRefused {
                    error_message: crate::connect::proto::PairingErrorMessage::NodeNotFound,
                    additional_info: None,
                }
            )),
            StatusCode::BAD_REQUEST
        );

        // And session initiation has its own two.
        let code = |e: ConnectError| session_error(e).status();
        assert_eq!(code(ConnectError::NotPaired), StatusCode::UNAUTHORIZED);
        assert_eq!(
            code(ConnectError::SessionRefused(
                crate::connect::proto::SessionRefused {
                    error_message: crate::connect::proto::SessionErrorMessage::NoLongerPaired,
                    additional_info: None,
                }
            )),
            StatusCode::BAD_REQUEST
        );
    }
}

#[cfg(test)]
mod subnet_wiring {
    use super::*;
    use tower::ServiceExt as _;

    fn endpoint() -> Endpoint {
        let store: Arc<dyn super::super::store::Store> = super::super::store::Memory::new();
        Endpoint::new(
            Arc::new(PairingService::lan(
                store.clone(),
                crate::connect::proto::EndpointDescription::default(),
                crate::connect::tls::Fingerprint::parse(&"AA".repeat(32)).expect("a fingerprint"),
            )),
            Arc::new(SessionService::new(store, "wss://example.com/s2")),
        )
    }

    async fn get(app: axum::Router, peer: Option<std::net::SocketAddr>) -> StatusCode {
        let mut request = axum::http::Request::builder()
            .uri("/v1/endpoint")
            .body(axum::body::Body::empty())
            .expect("a request");
        if let Some(peer) = peer {
            request
                .extensions_mut()
                .insert(axum::extract::ConnectInfo(peer));
        }
        app.oneshot(request)
            .await
            .expect("the router answers")
            .status()
    }

    #[tokio::test]
    async fn the_lan_routes_admit_a_local_peer_and_refuse_a_remote_one() {
        let subnets =
            LocalSubnets::from_prefixes([("192.168.1.10".parse().expect("an address"), 24)]);
        let app = lan_router(endpoint(), subnets);

        let local: std::net::SocketAddr = "192.168.1.50:9000".parse().expect("an address");
        assert_eq!(get(app.clone(), Some(local)).await, StatusCode::OK);

        let remote: std::net::SocketAddr = "8.8.8.8:9000".parse().expect("an address");
        assert_eq!(
            get(app.clone(), Some(remote)).await,
            StatusCode::UNAUTHORIZED
        );

        // No peer address at all means the server was not started with connect info.
        // Failing closed is the only safe reading: the alternative is serving the
        // unauthenticated routes to the internet because of a missing builder call.
        assert_eq!(get(app, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_wan_router_has_no_lan_routes_to_admit_anyone_to() {
        let app = router(endpoint());
        let local: std::net::SocketAddr = "127.0.0.1:9000".parse().expect("an address");
        // Absent, not refused: "It is recommended that WAN endpoints respond with status
        // code 404."
        assert_eq!(get(app, Some(local)).await, StatusCode::NOT_FOUND);
    }
}
