//! The thin layer between `reqwest` and the protocol: URLs, bearers, and what a status
//! code means.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::connect::proto::ConnectError;
use crate::connect::tls::{TlsClient, TlsPolicy};

/// What an S2 Connect endpoint answered.
///
/// The specification assigns meanings to particular codes on particular operations, and
/// they are not interchangeable — a `401` on `requestConnectionDetails` means "restart the
/// pairing", a `403` on the same operation means "this attempt is dead". Keeping the code
/// rather than collapsing everything into one error is what lets a driver tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u16);

impl Status {
    /// Whether the request succeeded.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    /// `401` — the bearer was not accepted.
    #[must_use]
    pub const fn is_unauthorized(self) -> bool {
        self.0 == 401
    }

    /// `403` — the challenge response was wrong; the attempt cannot proceed.
    #[must_use]
    pub const fn is_forbidden(self) -> bool {
        self.0 == 403
    }

    /// `503` — the server is rate-limiting; `S2C` says "try again soon".
    #[must_use]
    pub const fn is_busy(self) -> bool {
        self.0 == 503
    }
}

impl core::fmt::Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Anything that can go wrong talking to an S2 Connect endpoint.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The protocol refused to advance. Everything that is a *rule* surfaces here.
    #[error(transparent)]
    Protocol(#[from] ConnectError),
    /// TLS could not be set up, or the certificate could not be read.
    #[error(transparent)]
    Tls(#[from] crate::connect::tls::Error),
    /// The request never completed.
    #[error("{operation} failed: {source}")]
    Transport {
        /// Which operation.
        operation: &'static str,
        /// Why.
        source: Box<dyn core::error::Error + Send + Sync>,
    },
    /// The endpoint answered, but not with success.
    #[error("{operation} answered {status}{}", match .detail { Some(d) => alloc::format!(": {d}"), None => String::new() })]
    Http {
        /// Which operation.
        operation: &'static str,
        /// What it answered.
        status: Status,
        /// The body, when there was one worth keeping.
        detail: Option<String>,
    },
    /// The endpoint answered with success and a body we could not read.
    #[error("{operation} answered with a body that is not what the schema defines: {source}")]
    Body {
        /// Which operation.
        operation: &'static str,
        /// Why.
        source: serde_json::Error,
    },
    /// The URL is not one we can dial.
    #[error("{0} is not a usable URL")]
    Url(String),
    /// The certificate presented changed between two requests of one pairing attempt.
    #[error("the server's certificate changed during the pairing attempt")]
    IdentityChanged,
    /// The endpoint answered with more bytes than any S2 Connect body can legitimately be.
    #[error("{operation} answered with more than {max} bytes")]
    ResponseTooLarge {
        /// Which operation.
        operation: &'static str,
        /// The cap that was exceeded.
        max: usize,
    },
}

impl Error {
    /// Whether retrying after a back-off could plausibly work.
    ///
    /// A `503` is the specification's own "try again soon"; a transport failure may be a
    /// cable. Everything else is a decision, and repeating it will produce the same
    /// decision.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            // A cable, a DNS blip, a server still starting up.
            Error::Transport { .. } | Error::Protocol(ConnectError::RateLimited) => true,
            // `503` is the specification's own "try again soon"; every other code is a
            // decision, and repeating the request reproduces the decision.
            Error::Http { status, .. } => status.is_busy(),
            _ => false,
        }
    }
}

/// The most of an endpoint's answer that will be read into memory.
///
/// The symmetric half of [`MAX_BODY_BYTES`](crate::connect::server::MAX_BODY_BYTES), and
/// it matters for a reason that is easy to miss: during a LAN pairing the server is
/// deliberately **not** authenticated — that is the whole meaning of
/// [`TlsPolicy::LanPairingOnly`] — so the four pairing requests are answered by whatever
/// is on the other end of the socket, which on a LAN is anything that got there first.
/// Without a cap, a `reqwest` `text()` reads the lot.
///
/// Every S2 Connect body is a handful of identifiers, descriptions and Base64 challenges.
/// `GET /v1/nodes` is the largest, and it is a list of node descriptions. A quarter of a
/// mebibyte is four times what the server side accepts, so a long node list fits and a
/// hostile endpoint still cannot make a constrained Resource Manager hold a gigabyte.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// An HTTPS connection to one S2 Connect endpoint.
#[derive(Debug, Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    base: url::Url,
    tls: TlsClient,
}

impl Http {
    /// Open a connection to `base`, which must be the endpoint's versioned base URL.
    pub(crate) fn new(
        base: &str,
        policy: &TlsPolicy,
        timeout: core::time::Duration,
    ) -> Result<Self, Error> {
        let tls = TlsClient::new(policy)?;
        let base = normalize(base)?;
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(Arc::unwrap_or_clone(tls.config()))
            .timeout(timeout)
            // S2 Connect is HTTPS only; a redirect to plain HTTP would silently drop the
            // binding the whole protocol rests on.
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("s2-kit/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Transport {
                operation: "building the HTTPS client",
                source: Box::new(e),
            })?;
        Ok(Self { client, base, tls })
    }

    pub(crate) fn tls(&self) -> &TlsClient {
        &self.tls
    }

    pub(crate) fn base(&self) -> &url::Url {
        &self.base
    }

    /// `POST {base}{path}`, with an optional bearer, expecting a body back.
    pub(crate) async fn post<B: Serialize, R: DeserializeOwned>(
        &self,
        operation: &'static str,
        path: &str,
        bearer: Option<&str>,
        body: &B,
    ) -> Result<R, Error> {
        let text = self.post_raw(operation, path, bearer, Some(body)).await?;
        serde_json::from_str(&text).map_err(|source| Error::Body { operation, source })
    }

    /// `POST {base}{path}` with **no request body at all**, expecting one back.
    ///
    /// `confirmAccessToken` is the one operation `S2C-OAS session-init` defines with no
    /// `requestBody`: the bearer *is* the message. Posting `null` with a JSON content type
    /// — which is what serialising `()` produces — is a body the specification does not
    /// define, and a peer entitled to validate against the OpenAPI is entitled to refuse
    /// it.
    pub(crate) async fn post_bodyless<R: DeserializeOwned>(
        &self,
        operation: &'static str,
        path: &str,
        bearer: Option<&str>,
    ) -> Result<R, Error> {
        let url = self.join(path)?;
        let mut request = self.client.post(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| Error::Transport {
            operation,
            source: Box::new(e),
        })?;
        let text = self.read(operation, response).await?;
        serde_json::from_str(&text).map_err(|source| Error::Body { operation, source })
    }

    /// `POST {base}{path}` where the answer has no body worth reading.
    pub(crate) async fn post_empty<B: Serialize>(
        &self,
        operation: &'static str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&B>,
    ) -> Result<(), Error> {
        self.post_raw(operation, path, bearer, body).await.map(drop)
    }

    /// `GET {base}{path}`.
    pub(crate) async fn get<R: DeserializeOwned>(
        &self,
        operation: &'static str,
        path: &str,
    ) -> Result<R, Error> {
        let url = self.join(path)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Transport {
                operation,
                source: Box::new(e),
            })?;
        let text = self.read(operation, response).await?;
        serde_json::from_str(&text).map_err(|source| Error::Body { operation, source })
    }

    async fn post_raw<B: Serialize>(
        &self,
        operation: &'static str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&B>,
    ) -> Result<String, Error> {
        let url = self.join(path)?;
        let mut request = self.client.post(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|e| Error::Transport {
            operation,
            source: Box::new(e),
        })?;
        self.read(operation, response).await
    }

    async fn read(
        &self,
        operation: &'static str,
        response: reqwest::Response,
    ) -> Result<String, Error> {
        let status = Status(response.status().as_u16());
        let text = read_bounded(operation, response).await?;
        if status.is_success() {
            return Ok(text);
        }
        Err(Error::Http {
            operation,
            status,
            detail: (!text.is_empty()).then_some(text),
        })
    }

    fn join(&self, path: &str) -> Result<url::Url, Error> {
        self.base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::Url(alloc::format!("{}{path}", self.base)))
    }
}

/// Read a response body, refusing anything past [`MAX_RESPONSE_BYTES`].
///
/// Chunk by chunk rather than `Response::text()`, because `text()` has already allocated
/// the whole body by the time it returns a value anyone could measure. `Content-Length` is
/// checked first where the server sent one — it is a hint and not a promise, so the
/// running total is what actually enforces the cap.
async fn read_bounded(
    operation: &'static str,
    mut response: reqwest::Response,
) -> Result<String, Error> {
    let too_large = || Error::ResponseTooLarge {
        operation,
        max: MAX_RESPONSE_BYTES,
    };
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return Err(too_large());
    }

    let mut body: Vec<u8> = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(|e| Error::Transport {
            operation,
            source: Box::new(e),
        })?;
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    // Not `from_utf8_lossy`: a body that is not UTF-8 is not an S2 Connect body, and
    // replacing the bad bytes would hand the JSON parser a plausible-looking lie.
    String::from_utf8(body).map_err(|e| Error::Transport {
        operation,
        source: Box::new(e),
    })
}

/// Make a base URL joinable: HTTPS, and ending in a slash.
///
/// `url::Url::join` replaces the last path segment when the base does not end in one, so
/// `https://host/v1` + `requestPairing` gives `https://host/requestPairing` — an endpoint
/// that exists on no server and a bug that is very hard to see.
fn normalize(base: &str) -> Result<url::Url, Error> {
    let mut url = url::Url::parse(base).map_err(|_| Error::Url(base.to_string()))?;
    if url.scheme() != "https" {
        return Err(Error::Url(alloc::format!(
            "{base} is not HTTPS, and S2 Connect is HTTPS only"
        )));
    }
    if !url.path().ends_with('/') {
        let path = alloc::format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_url_without_a_trailing_slash_still_joins_correctly() {
        // Without this, `https://host/v1` + `requestPairing` resolves to
        // `https://host/requestPairing` — an endpoint that exists nowhere.
        let url = normalize("https://evse.local/v1").unwrap();
        assert_eq!(url.as_str(), "https://evse.local/v1/");
        assert_eq!(
            url.join("requestPairing").unwrap().as_str(),
            "https://evse.local/v1/requestPairing"
        );
        // And one that already has the slash is left alone.
        assert_eq!(
            normalize("https://evse.local/v1/").unwrap().as_str(),
            "https://evse.local/v1/"
        );
    }

    #[test]
    fn plain_http_is_refused_rather_than_upgraded() {
        // Silently upgrading would be worse: the caller would believe it had a binding.
        assert!(matches!(
            normalize("http://evse.local/v1/"),
            Err(Error::Url(_))
        ));
        assert!(matches!(normalize("ws://evse.local/"), Err(Error::Url(_))));
        assert!(matches!(normalize("not a url"), Err(Error::Url(_))));
    }

    #[test]
    fn the_statuses_the_specification_distinguishes_are_distinguished() {
        assert!(Status(200).is_success());
        assert!(Status(204).is_success());
        assert!(!Status(400).is_success());
        // "Provided pairingAttemptId not accepted. The client may restart the pairing
        // process" — recoverable.
        assert!(Status(401).is_unauthorized());
        // "The PairingAttempt has failed and cannot proceed" — not.
        assert!(Status(403).is_forbidden());
        // "The server is temporarily not able to process the pairing request, try again
        // soon."
        assert!(Status(503).is_busy());
    }

    #[test]
    fn only_the_errors_worth_retrying_say_they_are() {
        let busy = Error::Http {
            operation: "requestPairing",
            status: Status(503),
            detail: None,
        };
        assert!(busy.is_transient());
        let refused = Error::Http {
            operation: "requestPairing",
            status: Status(400),
            detail: None,
        };
        assert!(!refused.is_transient());
        assert!(!Error::IdentityChanged.is_transient());
        assert!(Error::Protocol(ConnectError::RateLimited).is_transient());
        assert!(!Error::Protocol(ConnectError::ChallengeFailed).is_transient());
    }
}
