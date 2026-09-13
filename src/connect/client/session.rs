//! Driving session initiation over HTTPS, and holding the connection open afterwards.

use alloc::string::ToString;
use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, Backoff, CommunicationDetails, NodeId, SessionCredentials, SessionGrant,
    SessionInitClient, TokenStore,
};
use crate::connect::tls::TlsPolicy;
use crate::types::{ProtocolVersion, Timestamp};

use super::http::{Error, Http};

const INITIATE_SESSION: &str = "initiateSession";
const CONFIRM_ACCESS_TOKEN: &str = "confirmAccessToken";
const UNPAIR: &str = "unpair";

/// Turns a stored pairing into an authorised WebSocket, as often as needed.
///
/// One `Session` is one pairing. It owns the token store, so the rotation the protocol
/// requires happens whether or not the caller remembers it exists — but the caller must
/// persist [`Session::tokens`] after every [`Session::initiate`], because a token that
/// only ever lived in memory is a pairing that does not survive a restart.
///
/// ```no_run
/// use s2_kit::connect::client::Session;
/// use s2_kit::connect::proto::*;
/// use s2_kit::connect::tls::TlsPolicy;
/// use s2_kit::types::Timestamp;
///
/// # async fn run(paired: s2_kit::connect::client::Paired) -> Result<(), Box<dyn std::error::Error>> {
/// let mut session = Session::new(
///     &paired.pairing.details.initiate_session_url,
///     &paired.policy(),
///     paired.pairing.local,
///     paired.pairing.remote.id,
///     TokenStore::new(paired.pairing.details.access_token.clone()),
/// )?;
///
/// let credentials = session.initiate(Timestamp::now()).await?;
/// // Persist the rotated tokens *now*, before connecting.
/// let to_store = session.tokens();
/// # let _ = (credentials, to_store);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Session {
    http: Http,
    machine: SessionInitClient,
    client_node_id: NodeId,
    server_node_id: NodeId,
    backoff: Backoff,
    attempt: u32,
}

impl Session {
    /// How long any single session-initiation request may take.
    ///
    /// Well inside the thirty seconds a communication token lives, so that a slow request
    /// cannot hand back credentials that have already expired.
    pub const REQUEST_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(10);

    /// A session for one pairing.
    ///
    /// `policy` should be [`Paired::policy`](super::Paired::policy) — the pinned CA for a
    /// LAN peer, the public PKI for a WAN one. Using [`TlsPolicy::LanPairingOnly`] here
    /// would leave the channel unauthenticated, so it is refused.
    pub fn new(
        base_url: &str,
        policy: &TlsPolicy,
        client_node_id: NodeId,
        server_node_id: NodeId,
        tokens: TokenStore,
    ) -> Result<Self, Error> {
        if !policy.authenticates() {
            return Err(Error::Url(
                "a session must not run under the pairing-only TLS policy".to_string(),
            ));
        }
        Ok(Self {
            http: Http::new(base_url, policy, Self::REQUEST_TIMEOUT)?,
            machine: SessionInitClient::new(client_node_id, server_node_id, tokens),
            client_node_id,
            server_node_id,
            backoff: Backoff::default(),
            attempt: 0,
        })
    }

    /// Offer a different set of S2 JSON message versions, newest first.
    #[must_use]
    pub fn with_s2_message_versions(mut self, versions: Vec<ProtocolVersion>) -> Self {
        let tokens = self.machine.tokens().clone();
        self.machine = SessionInitClient::new(self.client_node_id, self.server_node_id, tokens)
            .with_s2_message_versions(versions);
        self
    }

    /// The tokens, to persist. Read this after every [`Self::initiate`].
    #[must_use]
    pub fn tokens(&self) -> &TokenStore {
        self.machine.tokens()
    }

    /// Run both requests and come back with somewhere to connect.
    ///
    /// A `401` on the first is not a failure: the store may hold a token the server has
    /// already retired, so this works down the list the way `S2C §Recovery` prescribes,
    /// and only gives up when every one has been refused.
    pub async fn initiate(&mut self, now: Timestamp) -> Result<SessionCredentials, Error> {
        loop {
            let (bearer, body) = self.machine.initiate()?;
            let grant: SessionGrant = match self
                .http
                .post(
                    "initiateSession",
                    INITIATE_SESSION,
                    Some(bearer.as_str()),
                    &body,
                )
                .await
            {
                Ok(grant) => grant,
                Err(Error::Http { status, .. }) if status.is_unauthorized() => {
                    // Refused: try the next token, or conclude the pairing is gone.
                    crate::trace::event!(
                        warn,
                        remaining = self.machine.tokens().len(),
                        "the access token was refused; trying the next"
                    );
                    self.machine.unauthorized()?;
                    continue;
                }
                Err(e) => return Err(e),
            };

            self.machine.accept(grant, now)?;
            // The store now holds the replacement. A caller that persists here survives a
            // crash before the confirmation; one that does not, does not.
            let confirm = self.machine.confirm()?;
            // No request body: `S2C-OAS session-init/confirmAccessToken` defines none, and
            // the bearer is the whole message.
            let details: CommunicationDetails = self
                .http
                .post_bodyless(
                    "confirmAccessToken",
                    CONFIRM_ACCESS_TOKEN,
                    Some(confirm.as_str()),
                )
                .await?;
            let credentials = self.machine.connected(details, now)?;
            crate::trace::event!(
                info,
                url = %credentials.websocket_url,
                profile = ?credentials.profile,
                expires = %credentials.expires(),
                "session initiated"
            );
            self.attempt = 0;
            return Ok(credentials);
        }
    }

    /// Forget the pairing, telling the server so.
    ///
    /// The local store is cleared whether or not the server answers: a client that keeps
    /// a token for a pairing it has renounced is a client that will keep trying to use it.
    pub async fn unpair(&mut self) -> Result<(), Error> {
        let token = self.machine.tokens().current().cloned();
        let body = self.machine.unpaired();
        let Some(token) = token else {
            return Ok(());
        };
        self.http
            .post_empty("unpair", UNPAIR, Some(token.as_str()), Some(&body))
            .await
    }

    /// How long to wait before the next attempt, after one has failed.
    ///
    /// `S2C §Reconnection strategy`: `delay_n = random(0, min(600 s, 2 s × 2^n))`. The
    /// randomness is drawn here because a fleet of devices that all reconnect on the same
    /// schedule is a thundering herd; `Backoff::delay_with` takes it as a parameter for a
    /// test that wants it pinned.
    pub fn backoff(&mut self) -> core::time::Duration {
        let delay = self.backoff.delay_with(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        core::time::Duration::from(delay)
    }

    /// Give up on the current run of failures, so the next one starts from two seconds
    /// again.
    pub fn reset_backoff(&mut self) {
        self.attempt = 0;
    }

    /// A token that has been refused, so the next attempt does not start from the top.
    #[must_use]
    pub fn access_token(&self) -> Option<&AccessToken> {
        self.machine.tokens().current()
    }
}
