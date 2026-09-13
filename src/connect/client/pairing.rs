//! Driving the pairing state machine over HTTPS.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::connect::proto::{
    ConnectionDetails, Deployment, EndpointDescription, HmacBinding, NextStep, NodeDescription,
    Pairing as ProtoPairing, PairingClient, PairingCode,
};
use crate::connect::tls::{CapturedIdentity, TlsPolicy};
use crate::types::{ProtocolVersion, Timestamp};

use super::http::{Error, Http};

/// The paths, relative to the endpoint's versioned base URL.
const REQUEST_PAIRING: &str = "requestPairing";
const REQUEST_CONNECTION_DETAILS: &str = "requestConnectionDetails";
const POST_CONNECTION_DETAILS: &str = "postConnectionDetails";
// The path, which is what goes on the wire. `S2C-OAS pairing` gives this operation the
// `operationId` `confirmPairing`, which every other operation in the file does not do —
// so a generated client calls it `confirm_pairing` and nobody searching for
// `finalizePairing` finds it (erratum E28).
const FINALIZE_PAIRING: &str = "finalizePairing";

/// What a completed pairing leaves you holding.
#[derive(Debug)]
pub struct Paired {
    /// Everything the protocol agreed: who, where, and with which first access token.
    pub pairing: ProtoPairing,
    /// The certificate chain the server presented during pairing.
    ///
    /// For a LAN endpoint this is what the specification tells you to pin:
    /// `S2C §4` — "The client **must** pin the self-signed CA (root) certificate, and
    /// trust this certificate for the remainder of the pairing relation." Persist
    /// [`CapturedIdentity::root`] beside the access token; without it, every later
    /// connection to this device is unauthenticated.
    pub identity: Option<CapturedIdentity>,
}

impl Paired {
    /// The TLS policy every later connection to this peer should use.
    #[must_use]
    pub fn policy(&self) -> TlsPolicy {
        match &self.identity {
            Some(identity) => TlsPolicy::PinnedCa(identity.root.clone()),
            None => TlsPolicy::Web,
        }
    }
}

/// Runs one pairing attempt against one endpoint.
///
/// ```no_run
/// use s2_kit::connect::client::Pairing;
/// use s2_kit::connect::proto::*;
/// use s2_kit::types::Timestamp;
///
/// # async fn run(me: NodeDescription) -> Result<(), Box<dyn std::error::Error>> {
/// // Whatever the end user typed, and wherever they typed the URL from.
/// let code = PairingCode::parse("evse7-A1b2C3", TokenKind::Static)?;
///
/// let paired = Pairing::lan("https://EVSE1038.local/v1/", me, EndpointDescription::default())?
///     .run(code, Timestamp::now())
///     .await?;
///
/// // Persist both: the token is useless without the certificate to trust.
/// let policy = paired.policy();
/// # let _ = policy;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Pairing {
    http: Http,
    local: NodeDescription,
    local_endpoint: EndpointDescription,
    deployment: Deployment,
    /// `None` in a LAN, where the binding is the certificate we have not seen yet.
    domain: Option<String>,
    s2_message_versions: Option<Vec<ProtocolVersion>>,
    force: bool,
}

impl Pairing {
    /// How long any single pairing request may take.
    ///
    /// Shorter than the fifteen-second budget for the whole attempt, so that one slow
    /// request fails as a request rather than as an expired pairing — a far clearer thing
    /// to see in a log.
    pub const REQUEST_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(10);

    /// Pair with an endpoint in the same LAN.
    ///
    /// The certificate is self-signed and unverifiable, so the first request runs under
    /// [`TlsPolicy::LanPairingOnly`] and the challenge response is what decides whether
    /// the server was genuine. See [`crate::connect::tls`].
    pub fn lan(
        base_url: &str,
        local: NodeDescription,
        local_endpoint: EndpointDescription,
    ) -> Result<Self, Error> {
        Ok(Self {
            http: Http::new(base_url, &TlsPolicy::LanPairingOnly, Self::REQUEST_TIMEOUT)?,
            local,
            local_endpoint,
            deployment: Deployment::Lan,
            domain: None,
            s2_message_versions: None,
            force: false,
        })
    }

    /// Pair with an endpoint on the public internet.
    ///
    /// Here the certificate *is* verifiable, so it is verified, and the binding is the
    /// domain name rather than a fingerprint.
    pub fn wan(
        base_url: &str,
        local: NodeDescription,
        local_endpoint: EndpointDescription,
    ) -> Result<Self, Error> {
        let http = Http::new(base_url, &TlsPolicy::Web, Self::REQUEST_TIMEOUT)?;
        // "The domain name of the HTTPS server, including subdomains, without protocol or
        // trailing slashes."
        let domain = http
            .base()
            .host_str()
            .ok_or_else(|| Error::Url(base_url.to_string()))?
            .to_string();
        Ok(Self {
            http,
            local,
            local_endpoint,
            deployment: Deployment::Wan,
            domain: Some(domain),
            s2_message_versions: None,
            force: false,
        })
    }

    /// Offer a different set of S2 JSON message versions, newest first.
    #[must_use]
    pub fn with_s2_message_versions(mut self, versions: Vec<ProtocolVersion>) -> Self {
        self.s2_message_versions = Some(versions);
        self
    }

    /// Pair even with no S2 message version in common.
    #[must_use]
    pub fn forcing(mut self) -> Self {
        self.force = true;
        self
    }

    /// Run the whole attempt: four requests, or three plus a push.
    ///
    /// `now` starts the fifteen-second budget. Entropy is drawn from the operating system;
    /// [`Self::run_with_entropy`] takes it as a parameter for a test that needs it pinned.
    pub async fn run(&self, code: PairingCode, now: Timestamp) -> Result<Paired, Error> {
        let mut nonce = [0u8; 32];
        fill_random(&mut nonce)?;
        self.run_with_entropy(code, now, &nonce, None).await
    }

    /// As [`Self::run`], with the client nonce supplied and, when this node will be the
    /// communication server, the details to offer.
    ///
    /// A LAN Resource Manager never needs the second argument: it is never the
    /// communication server. A CEM usually does.
    pub async fn run_with_entropy(
        &self,
        code: PairingCode,
        now: Timestamp,
        client_nonce: &[u8],
        own_details: Option<ConnectionDetails>,
    ) -> Result<Paired, Error> {
        // Step 1 carries a challenge and no response, which is exactly why it can be
        // sent before the binding is known: in a LAN, the handshake this request performs
        // is what reveals it.
        let mut machine = self.machine(code, now);
        let body = machine.request_pairing(client_nonce)?;
        crate::trace::event!(
            info,
            deployment = ?self.deployment,
            alias = ?body.node_id_alias,
            "requesting pairing"
        );
        let accepted: crate::connect::proto::PairingAccepted = self
            .http
            .post("requestPairing", REQUEST_PAIRING, None, &body)
            .await?;

        // From here on the server is holding an attempt for us, and every failure below is
        // one it would otherwise sit on for the whole fifteen-second budget — refusing
        // every retry with `503`, because the rate limit is one attempt per node per
        // second and an attempt in flight occupies the slot. `finalizePairing { success:
        // false }` is what releases it, and `S2C` calls that a legitimate outcome rather
        // than an error path. So the rest of the attempt runs in a closure whose failure
        // is reported *and* cleaned up.
        let bearer_of = |machine: &PairingClient| {
            machine
                .attempt_id()
                .map(|id| id.as_str().to_string())
                .unwrap_or_default()
        };
        match self.finish(&mut machine, accepted, now, own_details).await {
            Ok(paired) => Ok(paired),
            Err(e) => {
                let bearer = bearer_of(&machine);
                if !bearer.is_empty() {
                    crate::trace::event!(
                        info,
                        error = %e,
                        "releasing the pairing attempt after a failure"
                    );
                    self.abandon(&bearer).await;
                }
                Err(e)
            }
        }
    }

    /// Steps 4 to 8, once `requestPairing` has been answered.
    async fn finish(
        &self,
        machine: &mut PairingClient,
        accepted: crate::connect::proto::PairingAccepted,
        now: Timestamp,
        own_details: Option<ConnectionDetails>,
    ) -> Result<Paired, Error> {
        // The handshake has happened, so in a LAN we now know `F`. Supplying it here —
        // rather than at the start — is the whole reason this driver exists.
        let identity = self.http.tls().captured();
        if !self.http.tls().identity_is_stable() {
            return Err(Error::IdentityChanged);
        }
        let binding = match &self.domain {
            Some(domain) => HmacBinding::DomainName(domain.clone()),
            None => identity
                .as_ref()
                .map(|i| i.leaf.binding())
                .ok_or(Error::IdentityChanged)?,
        };

        crate::trace::event!(debug, binding = ?binding, "checking the server's response");

        // Steps 4 and 5.
        let next = machine.accept(accepted, binding, self.deployment, now)?;
        crate::trace::event!(info, next = ?next, "the server authenticated");
        let bearer = machine
            .attempt_id()
            .map(|id| id.as_str().to_string())
            .unwrap_or_default();

        // Steps 6 and 7.
        match next {
            NextStep::RequestConnectionDetails => {
                let ask = machine.request_connection_details()?;
                let details: ConnectionDetails = self
                    .http
                    .post(
                        "requestConnectionDetails",
                        REQUEST_CONNECTION_DETAILS,
                        Some(&bearer),
                        &ask,
                    )
                    .await?;
                machine.connection_details(details, now)?;
            }
            NextStep::PostConnectionDetails => {
                let details = own_details.ok_or(Error::Protocol(
                    crate::connect::proto::ConnectError::OutOfOrder {
                        got: "a pairing in which this node is the communication server",
                        expected: "connection details to offer",
                    },
                ))?;
                let post = machine.post_connection_details(details, now)?;
                self.http
                    .post_empty(
                        "postConnectionDetails",
                        POST_CONNECTION_DETAILS,
                        Some(&bearer),
                        Some(&post),
                    )
                    .await?;
            }
        }

        // Step 8. The certificate must still be the one we bound to.
        if !self.http.tls().identity_is_stable() {
            return Err(Error::IdentityChanged);
        }
        let (body, pairing) = machine.finalize(now)?;
        crate::trace::event!(
            info,
            remote = %pairing.remote.id,
            communication_server = ?pairing.communication_server,
            "pairing complete"
        );
        self.http
            .post_empty(
                "finalizePairing",
                FINALIZE_PAIRING,
                Some(&bearer),
                Some(&body),
            )
            .await?;

        Ok(Paired {
            pairing,
            identity: (self.deployment == Deployment::Lan)
                .then_some(identity)
                .flatten(),
        })
    }

    /// Tell the server we are giving up, so it does not hold the attempt open.
    ///
    /// Best effort: if this fails there is nothing useful to do about it, and the server's
    /// own fifteen-second budget will clean up.
    pub async fn abandon(&self, bearer: &str) {
        let body = crate::connect::proto::FinalizePairing { success: false };
        let _ = self
            .http
            .post_empty(
                "finalizePairing",
                FINALIZE_PAIRING,
                Some(bearer),
                Some(&body),
            )
            .await;
    }

    fn machine(&self, code: PairingCode, now: Timestamp) -> PairingClient {
        let mut machine =
            PairingClient::start(self.local.clone(), self.local_endpoint.clone(), code, now);
        if let Some(versions) = &self.s2_message_versions {
            machine = machine.with_s2_message_versions(versions.clone());
        }
        if self.force {
            machine = machine.forcing();
        }
        machine
    }
}

/// Fill a buffer with cryptographically secure randomness.
pub(crate) fn fill_random(buffer: &mut [u8]) -> Result<(), Error> {
    use rand::TryRng as _;
    // The operating system's CSPRNG. A failure here means the system has no entropy
    // source — not something a retry fixes, and not something to paper over with a
    // weaker generator when the value is about to authenticate a device.
    rand::rngs::SysRng
        .try_fill_bytes(buffer)
        .map_err(|e| Error::Transport {
            operation: "drawing randomness",
            source: Box::new(e),
        })
}
