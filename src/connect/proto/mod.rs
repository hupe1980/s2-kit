//! S2 Connect as pure state machines: no sockets, no clock, no hidden randomness.
//!
//! Everything a driver could forget lives here instead — the HMAC binding that defeats a
//! man in the middle, the fifteen-second pairing budget, the one-attempt-per-node-per-
//! second rate limit, the pending-token exchange that must not strand a pairing if the
//! network drops between two requests.

pub mod lan;
pub mod pairing;
pub mod session_init;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::types::Duration;

pub use lan::{
    CancelPreparePairing, LONG_POLL_HOLD, LONG_POLL_TIMEOUT, NodeListing, PreparePairing,
    PrepareRefused, WaitAction, WaitErrorMessage, WaitForPairing, WaitInstruction,
};
pub use pairing::{
    ConnectionDetails, ConnectionDetailsPost, ConnectionDetailsRequest, FinalizePairing,
    MINIMUM_SERVER_DELAY, NextStep, PAIRING_BUDGET, Pairing, PairingAccepted, PairingAttemptId,
    PairingClient, PairingErrorMessage, PairingRefused, PairingRequest, PairingServer, PairingStep,
};
pub use session_init::{
    COMMUNICATION_TOKEN_LIFETIME, CommunicationDetails, SessionCredentials, SessionErrorMessage,
    SessionGrant, SessionInitClient, SessionInitServer, SessionRefused, SessionRequest, TokenStore,
    UnpairRequest,
};

/// Which side of an S2 conversation a node hosts.
///
/// The same two values as [`EnergyManagementRole`](crate::types::common::EnergyManagementRole),
/// spelled as `s2-connect-common.yml` spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Role {
    /// A Customer Energy Manager.
    Cem,
    /// A Resource Manager.
    Rm,
}

impl Role {
    /// The DNS-SD subtype a node of this role advertises.
    #[must_use]
    pub const fn service_subtype(self) -> &'static str {
        match self {
            Role::Cem => "_cem",
            Role::Rm => "_rm",
        }
    }

    /// Whether these two roles may pair. "S2 communication between two nodes can only be
    /// established if one of the nodes is a CEM and the other a RM."
    #[must_use]
    pub const fn can_pair_with(self, other: Self) -> bool {
        !matches!((self, other), (Role::Cem, Role::Cem) | (Role::Rm, Role::Rm))
    }

    /// The only role this one can pair with.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Role::Cem => Role::Rm,
            Role::Rm => Role::Cem,
        }
    }
}

/// Where a node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Deployment {
    /// On the public internet.
    Wan,
    /// On the building's own network.
    Lan,
}

impl Deployment {
    /// Which side is the **communication** server, given both deployments and which is
    /// the CEM.
    ///
    /// `S2C §Mapping the CEM and RM to communication server or client`: a WAN node is
    /// always the server; otherwise the CEM is.
    #[must_use]
    pub const fn communication_server(cem: Self, rm: Self) -> Role {
        match (cem, rm) {
            (Deployment::Lan, Deployment::Wan) => Role::Rm,
            _ => Role::Cem,
        }
    }
}

/// A node's globally unique identifier.
///
/// `s2-connect-common.yml/NodeId` gives `format: uuid`, and [`NodeId::generate`] produces
/// one. The underlying [`Id`](crate::types::Id) is the S2 JSON identifier type, which
/// accepts more than a UUID — so a peer that sends `"evse-1"` is understood rather than
/// refused, on the same reasoning as erratum E1.
///
/// Ordered so that an endpoint can key a map by it, which every endpoint hosting more
/// than one node needs to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub crate::types::Id);

impl NodeId {
    /// A fresh identifier.
    #[cfg(feature = "uuid")]
    #[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
    #[must_use]
    pub fn generate() -> Self {
        Self(crate::types::Id::generate())
    }

    /// Parse one.
    pub fn parse(s: &str) -> Result<Self, ConnectError> {
        crate::types::Id::parse(s)
            .map(Self)
            .map_err(|_| ConnectError::MalformedNodeId)
    }
}

impl core::fmt::Display for NodeId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// A short identifier for a node within one endpoint.
///
/// An endpoint may host thousands of nodes; a node id is a UUID and nobody is going to
/// type one. The alias is what makes the pairing code short enough to read off a screen.
///
/// The invariant is enforced on the way **in as well as out**: `nodeIdAlias` is a field of
/// `requestPairing`, which needs no bearer, so it is a string an unauthenticated peer
/// chooses. A `#[serde(transparent)]` newtype whose `parse` validates and whose
/// `Deserialize` does not is a validated type in name only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct NodeIdAlias(pub String);

impl NodeIdAlias {
    /// The longest alias this crate accepts.
    ///
    /// The specification's pattern, `^[0-9a-zA-Z]+$`, has no upper bound — but the alias
    /// exists so that a person can read a pairing code off a screen and type it, and
    /// nothing about that is improved past sixty-four characters. A bound is what turns
    /// "an unauthenticated peer picks this string" from a question into a fact.
    pub const MAX_LEN: usize = 64;

    /// Parse an alias: `^[0-9a-zA-Z]+$`, at most [`MAX_LEN`](Self::MAX_LEN) characters.
    pub fn parse(s: &str) -> Result<Self, ConnectError> {
        if s.is_empty() || s.len() > Self::MAX_LEN || !s.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(ConnectError::MalformedPairingCode);
        }
        Ok(Self(s.to_string()))
    }
}

impl<'de> Deserialize<'de> for NodeIdAlias {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `String`, not `&str`: a borrowed string cannot come out of a `serde_json::Value`
        // at all, so a `&str` here refuses every body that anything re-parsed — a proxy, a
        // middleware, a test fixture — with "expected a borrowed string", which reads as a
        // malformed alias rather than as a deserializer that cannot borrow. The same trap
        // `codec::peek` documents for `message_type`, on a field an unauthenticated peer
        // chooses.
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// The secret half of a pairing code.
///
/// Zeroized on drop and never compared with `==`: [`ChallengeResponse`] is the only thing
/// that ever looks at it, and it does so in constant time.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PairingToken(Vec<u8>);

/// Compared in constant time, so that a `==` on a secret cannot leak its prefix through
/// timing. `PairingCode` derives `PartialEq`, and this is what that derive calls.
impl PartialEq for PairingToken {
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq as _;
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for PairingToken {}

impl PairingToken {
    /// The fewest characters a token that expires may have.
    pub const MIN_DYNAMIC: usize = 4;
    /// The fewest characters a token printed on a device may have.
    pub const MIN_STATIC: usize = 6;
    /// The most characters any pairing token may have.
    ///
    /// The specification gives minimums and no maximum. A token is typed by a person or
    /// read off a label, so 256 characters is already absurd — and the value goes into an
    /// HMAC, so an unbounded one is unbounded work for whoever holds the other end.
    pub const MAX_LEN: usize = 256;

    /// Parse a token, checking the length its kind requires.
    pub fn parse(s: &str, kind: TokenKind) -> Result<Self, ConnectError> {
        let minimum = match kind {
            TokenKind::Dynamic => Self::MIN_DYNAMIC,
            TokenKind::Static => Self::MIN_STATIC,
        };
        if s.len() < minimum
            || s.len() > Self::MAX_LEN
            || !s.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(ConnectError::MalformedPairingCode);
        }
        Ok(Self(s.as_bytes().to_vec()))
    }

    /// The bytes, for the HMAC. `S2C §Challenge response process`: "The pairing token and
    /// domain name are strings, which need to be converted into binary data using the
    /// ASCII table."
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl core::fmt::Debug for PairingToken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never print a secret, not even in a panic message.
        write!(f, "PairingToken(<{} characters>)", self.0.len())
    }
}

/// Whether a pairing token expires.
///
/// # A four-character token is weaker than it looks
///
/// The specification's stated mitigation for brute force is the server's one-second delay
/// and its per-node serialisation ([`pairing::MINIMUM_SERVER_DELAY`]), and this crate
/// implements both. They bound *online* guessing.
///
/// They do not bound offline guessing, because `requestPairing` answers with
/// `R_c = HMAC(C_c, T ‖ F)` for a challenge the caller chose, and `F` — the server's
/// leaf-certificate fingerprint — is public. One reachable request is therefore enough to
/// recover the token at leisure: a [`Dynamic`](Self::Dynamic) token's four alphanumeric
/// characters are 14.7 million candidates, which is seconds of GPU time, and its five
/// minutes of validity do not help against an attacker who already has the answer.
///
/// So: prefer longer tokens than the minimum, and treat the minimum as what it is — a
/// floor the specification permits, recorded as erratum E18.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Generated on demand and expiring; five minutes is recommended. At least
    /// [`PairingToken::MIN_DYNAMIC`] characters — see the warning above about that floor.
    Dynamic,
    /// Printed on the device, valid indefinitely. At least
    /// [`PairingToken::MIN_STATIC`] characters.
    Static,
}

/// What the end user copies from one node to the other.
///
/// `[nodeIdAlias-]pairingToken`, matching `^([0-9a-zA-Z]+-)?[0-9a-zA-Z]{4,}$`. The alias
/// is not secret and is sent; the token is secret and never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    /// Which node at the endpoint, when the endpoint hosts more than one.
    pub alias: Option<NodeIdAlias>,
    /// The secret.
    pub token: PairingToken,
}

impl PairingCode {
    /// Parse a pairing code as the end user typed it.
    ///
    /// ```
    /// use s2_kit::connect::proto::{PairingCode, TokenKind};
    ///
    /// // An endpoint with one node: the code is just the token.
    /// let code = PairingCode::parse("A1b2C3", TokenKind::Static)?;
    /// assert!(code.alias.is_none());
    ///
    /// // An endpoint with many: the alias comes first.
    /// let code = PairingCode::parse("evse7-A1b2C3", TokenKind::Static)?;
    /// assert_eq!(code.alias.unwrap().0, "evse7");
    ///
    /// // The token is checked for length, so a typo is caught before the network is.
    /// assert!(PairingCode::parse("abc", TokenKind::Dynamic).is_err());
    /// # Ok::<(), s2_kit::connect::proto::ConnectError>(())
    /// ```
    pub fn parse(code: &str, kind: TokenKind) -> Result<Self, ConnectError> {
        match code.split_once('-') {
            // A dash means an alias precedes the token — but a token may not contain one,
            // so the *first* dash is the separator.
            Some((alias, token)) => Ok(Self {
                alias: Some(NodeIdAlias::parse(alias)?),
                token: PairingToken::parse(token, kind)?,
            }),
            None => Ok(Self {
                alias: None,
                token: PairingToken::parse(code, kind)?,
            }),
        }
    }
}

/// A one-time secret that lets a communication client start a session.
///
/// `S2C-OAS common`: "valid for one time login, with a maximum 5 years", Base64, at least
/// 32 bytes.
#[derive(Clone, Zeroize, ZeroizeOnDrop, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccessToken(String);

/// A secret that authorises exactly one WebSocket connection.
///
/// `S2C-OAS common`: "valid for maximum 30 seconds". The prose calls it `commToken`; the
/// schema calls it `websocketToken` (erratum E9).
#[derive(Clone, Zeroize, ZeroizeOnDrop, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommunicationToken(String);

macro_rules! opaque_token {
    ($ty:ident, $name:literal, $lifetime:expr) => {
        impl $ty {
            /// The fewest bytes of entropy the specification requires.
            pub const MIN_BYTES: usize = 32;

            /// Wrap a token received from a peer.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Generate one from 32 bytes of cryptographically secure randomness.
            ///
            /// The bytes are a parameter rather than drawn here, so the protocol layer
            /// stays pure and a test can pin them.
            pub fn from_entropy(bytes: &[u8]) -> Result<Self, ConnectError> {
                if bytes.len() < Self::MIN_BYTES {
                    return Err(ConnectError::WeakSecret {
                        got: bytes.len(),
                        need: Self::MIN_BYTES,
                    });
                }
                Ok(Self(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                ))
            }

            /// The Base64 form, which is what travels.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// How long the specification says it stays usable.
            #[must_use]
            pub const fn lifetime() -> Duration {
                $lifetime
            }

            /// Whether another token is this one, compared in constant time.
            #[must_use]
            pub fn verify(&self, other: &Self) -> bool {
                self.verify_str(&other.0)
            }

            /// Whether a token as it arrived on the wire is this one, compared in
            /// constant time.
            ///
            /// A short-circuiting `==` on a bearer token leaks its prefix to anyone who
            /// can time the answer, which is why these types have no `PartialEq` at all:
            /// there is no way to compare them carelessly.
            #[must_use]
            pub fn verify_str(&self, presented: &str) -> bool {
                use subtle::ConstantTimeEq as _;
                // Length is not a secret — the encoding fixes it — so an early return on
                // a mismatch reveals nothing a listener could not already count.
                self.0.len() == presented.len()
                    && bool::from(self.0.as_bytes().ct_eq(presented.as_bytes()))
            }
        }

        impl core::fmt::Debug for $ty {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!($name, "(<redacted>)"))
            }
        }
    };
}

// 5 years and 30 seconds, as `s2-connect-common.yml` gives them.
opaque_token!(AccessToken, "AccessToken", Duration::from_secs(157_680_000));
opaque_token!(
    CommunicationToken,
    "CommunicationToken",
    Duration::from_secs(30)
);

/// Information about a node, as `s2-connect-common.yml` defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescription {
    /// The node's identifier.
    pub id: NodeId,
    /// The brand a user would recognise.
    pub brand: String,
    /// A logo.
    #[serde(rename = "logoUrl", skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    /// What kind of thing it is.
    #[serde(rename = "type")]
    pub kind: String,
    /// The model name.
    #[serde(rename = "modelName")]
    pub model_name: String,
    /// A name the user gave it.
    #[serde(rename = "userDefinedName", skip_serializing_if = "Option::is_none")]
    pub user_defined_name: Option<String>,
    /// Whether it is a CEM or an RM.
    pub role: Role,
}

/// Information about the endpoint that hosts nodes. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointDescription {
    /// A user-facing name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A logo.
    #[serde(rename = "logoUrl", skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    /// Where it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<Deployment>,
}

/// Anything that can go wrong in the pure protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectError {
    /// The pairing code does not match `^([0-9a-zA-Z]+-)?[0-9a-zA-Z]{4,}$`, or the token
    /// is shorter than its kind allows.
    #[error("the pairing code is malformed")]
    MalformedPairingCode,
    /// A node identifier is not a legal identifier.
    #[error("the node identifier is malformed")]
    MalformedNodeId,
    /// A challenge or token carries less entropy than the specification requires.
    #[error("a secret of {got} bytes is weaker than the {need} the specification requires")]
    WeakSecret {
        /// What was supplied.
        got: usize,
        /// What is required.
        need: usize,
    },
    /// The challenge response did not match. Someone is in the middle, or the user
    /// mistyped.
    #[error("the challenge response does not match")]
    ChallengeFailed,
    /// The two nodes cannot pair: two CEMs, or two RMs.
    #[error("a {0:?} cannot pair with another {0:?}")]
    SameRole(Role),
    /// The step arrived out of order.
    #[error("{got} arrived while waiting for {expected}")]
    OutOfOrder {
        /// What arrived.
        got: &'static str,
        /// What was expected.
        expected: &'static str,
    },
    /// The fifteen-second budget ran out.
    #[error("the {what} budget of {} seconds ran out", limit.as_secs_f64())]
    Expired {
        /// Which budget.
        what: &'static str,
        /// How long it was.
        limit: Duration,
    },
    /// The server is already handling an attempt for this node.
    ///
    /// `S2C §2. Calculate clientHmacChallengeResponse`: "pairing attempts **must** be
    /// handled sequentially, such that each second only one pairing attempt can be
    /// processed for a node."
    #[error("another pairing attempt for this node is in progress")]
    RateLimited,
    /// The peer offered no version, protocol or algorithm in common.
    #[error("no common {0}")]
    NoOverlap(&'static str),
    /// No pairing exists for this pair of node identifiers, or the token is wrong.
    #[error("not paired")]
    NotPaired,
    /// The pairing server refused, with one of the reasons the specification enumerates.
    ///
    /// A driver turns this into `400` with the body verbatim.
    #[error("pairing refused: {:?}", .0.error_message)]
    Refused(pairing::PairingRefused),
    /// The communication server refused a session, likewise.
    #[error("session refused: {:?}", .0.error_message)]
    SessionRefused(session_init::SessionRefused),
}

/// What the challenge–response is bound to, which is what defeats a man in the middle.
///
/// `S2C §Challenge response process`:
/// `R = HMAC(C, T ‖ F)` when the pairing server is in the LAN, where `F` is the SHA-256
/// fingerprint of the **leaf** TLS certificate; `R = HMAC(C, T ‖ D)` in the WAN, where
/// `D` is the domain name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HmacBinding {
    /// A LAN server: the fingerprint of its leaf certificate, already hex-decoded.
    ///
    /// "SHA256 certificate fingerprints are encoded into a hexadecimal string, and must
    /// be decoded as hexadecimal string before it can be used as input (note that
    /// fingerprint strings usually contain colons to separate bytes)."
    LeafFingerprint(Vec<u8>),
    /// A WAN server: its domain name, "including subdomains, without protocol or trailing
    /// slashes".
    DomainName(String),
}

impl HmacBinding {
    /// A binding from a fingerprint written as hexadecimal, with or without colons.
    pub fn from_hex_fingerprint(hex: &str) -> Result<Self, ConnectError> {
        let digits: Vec<u8> = hex.bytes().filter(|b| *b != b':' && *b != b' ').collect();
        if !digits.len().is_multiple_of(2) || digits.is_empty() {
            return Err(ConnectError::WeakSecret {
                got: digits.len() / 2,
                need: 32,
            });
        }
        let mut bytes = Vec::with_capacity(digits.len() / 2);
        for pair in digits.chunks(2) {
            let hi = hex_value(*pair.first().unwrap_or(&0))?;
            let lo = hex_value(*pair.get(1).unwrap_or(&0))?;
            bytes.push(hi << 4 | lo);
        }
        if bytes.len() < 32 {
            return Err(ConnectError::WeakSecret {
                got: bytes.len(),
                need: 32,
            });
        }
        Ok(Self::LeafFingerprint(bytes))
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            HmacBinding::LeafFingerprint(bytes) => bytes,
            HmacBinding::DomainName(name) => name.as_bytes(),
        }
    }
}

fn hex_value(b: u8) -> Result<u8, ConnectError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(ConnectError::MalformedPairingCode),
    }
}

/// How S2 messages travel once a session is initiated.
///
/// `s2-connect-common.yml/CommunicationProtocol`. One value today; the enumeration exists
/// because the specification expects more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CommunicationProtocol {
    /// A WebSocket, which is the only transport S2 Connect 1.0.0 defines.
    WebSocket,
}

/// The only hash the specification currently allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum HmacHashingAlgorithm {
    /// SHA-256, which "**must** be present" in every client's list.
    #[serde(rename = "SHA256")]
    Sha256,
}

/// A challenge and its answer.
///
/// The challenge is a nonce of at least 32 random bytes; the answer is
/// `HMAC(key = challenge, message = token ‖ binding)`. Both sides compute it, and the one
/// that issued the challenge compares — in constant time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeResponse {
    bytes: [u8; 32],
}

impl ChallengeResponse {
    /// Compute the response to a challenge.
    ///
    /// ```
    /// use s2_kit::connect::proto::{ChallengeResponse, HmacBinding, PairingCode, TokenKind};
    ///
    /// let code = PairingCode::parse("A1b2C3", TokenKind::Static)?;
    /// let challenge = [7u8; 32];
    /// let binding = HmacBinding::DomainName("pairing.example.com".into());
    ///
    /// // Both sides compute the same answer from the same three inputs.
    /// let theirs = ChallengeResponse::compute(&challenge, &code.token, &binding)?;
    /// let ours = ChallengeResponse::compute(&challenge, &code.token, &binding)?;
    /// assert!(ours.verify(theirs.as_base64().as_str()));
    ///
    /// // A different binding — a man in the middle with his own certificate — does not.
    /// let elsewhere = HmacBinding::DomainName("attacker.example.com".into());
    /// let forged = ChallengeResponse::compute(&challenge, &code.token, &elsewhere)?;
    /// assert!(!ours.verify(forged.as_base64().as_str()));
    /// # Ok::<(), s2_kit::connect::proto::ConnectError>(())
    /// ```
    pub fn compute(
        challenge: &[u8],
        token: &PairingToken,
        binding: &HmacBinding,
    ) -> Result<Self, ConnectError> {
        use hmac::Mac as _;
        if challenge.len() < 32 {
            return Err(ConnectError::WeakSecret {
                got: challenge.len(),
                need: 32,
            });
        }
        let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(challenge)
            .map_err(|_| ConnectError::ChallengeFailed)?;
        mac.update(token.as_bytes());
        mac.update(binding.as_bytes());
        let out = mac.finalize().into_bytes();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&out);
        Ok(Self { bytes })
    }

    /// Parse a response received from a peer.
    pub fn from_base64(encoded: &str) -> Result<Self, ConnectError> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| ConnectError::ChallengeFailed)?;
        if decoded.len() != 32 {
            return Err(ConnectError::ChallengeFailed);
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self { bytes })
    }

    /// The Base64 form, which is what travels.
    #[must_use]
    pub fn as_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.bytes)
    }

    /// Whether a peer's answer matches ours, compared in constant time.
    #[must_use]
    pub fn verify(&self, theirs: &str) -> bool {
        use subtle::ConstantTimeEq as _;
        let Ok(other) = Self::from_base64(theirs) else {
            return false;
        };
        self.bytes.ct_eq(&other.bytes).into()
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn base64_decode(encoded: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
}

/// The reconnection delay S2 Connect prescribes.
///
/// `S2C §Reconnection strategy`: `delay_n = random(0, min(max_delay, base_delay × 2^n))`,
/// with a base of two seconds and a maximum of six hundred.
pub type Backoff = crate::session::Backoff;

/// The DNS-SD service S2 Connect uses.
pub mod dnssd {
    use super::{Deployment, Role};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// `_s2connect`, always over TCP because the protocol is HTTPS.
    pub const SERVICE_TYPE: &str = "_s2connect";
    /// The transport the service type is registered under.
    pub const PROTOCOL: &str = "_tcp";
    /// The value `txtver` must carry for this version of the specification.
    pub const TXT_VERSION: &str = "1";

    /// What an endpoint advertises.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ServiceRecord {
        /// A user-facing name for the endpoint.
        pub endpoint_name: Option<String>,
        /// A logo.
        pub endpoint_logo_url: Option<String>,
        /// The base URL of the pairing API, including the trailing slash.
        pub pairing_url: Option<String>,
        /// The base URL of the long-polling API, for endpoints that host only Resource
        /// Managers and implement no HTTPS server.
        pub longpolling_url: Option<String>,
        /// Which roles this endpoint hosts, as DNS-SD subtypes.
        pub roles: Vec<Role>,
    }

    impl ServiceRecord {
        /// The TXT record, as key–value pairs in the order the specification lists them.
        ///
        /// The key is `txtver`, which is what the normative table says and what the only
        /// other implementation emits — although the avahi example on the same page of
        /// the specification writes `txtvers` (erratum E7).
        #[must_use]
        pub fn txt_pairs(&self) -> Vec<(&'static str, String)> {
            let mut out = alloc::vec![("txtver", TXT_VERSION.to_string())];
            if let Some(name) = &self.endpoint_name {
                out.push(("e_name", name.clone()));
            }
            if let Some(logo) = &self.endpoint_logo_url {
                out.push(("e_logoUrl", logo.clone()));
            }
            if let Some(url) = &self.pairing_url {
                out.push(("pairingUrl", url.clone()));
            }
            if let Some(url) = &self.longpolling_url {
                out.push(("longpollingUrl", url.clone()));
            }
            out
        }

        /// Read a record from a browsed TXT record.
        ///
        /// Both spellings of the version key are accepted, because both appear in the
        /// specification.
        #[must_use]
        pub fn from_txt<'a, I>(pairs: I) -> Option<Self>
        where
            I: IntoIterator<Item = (&'a str, &'a str)>,
        {
            let mut record = Self::default();
            let mut version_seen = false;
            for (key, value) in pairs {
                match key {
                    "txtver" | "txtvers" => {
                        if value != TXT_VERSION {
                            return None;
                        }
                        version_seen = true;
                    }
                    "e_name" => record.endpoint_name = Some(value.to_string()),
                    "e_logoUrl" => record.endpoint_logo_url = Some(value.to_string()),
                    "pairingUrl" => record.pairing_url = Some(value.to_string()),
                    "longpollingUrl" => record.longpolling_url = Some(value.to_string()),
                    _ => {}
                }
            }
            // "It is mandatory to provide a value for at least one of the properties
            // pairingUrl and longpollingUrl."
            if !version_seen || (record.pairing_url.is_none() && record.longpolling_url.is_none()) {
                return None;
            }
            Some(record)
        }

        /// Whether this endpoint can be paired with directly, as opposed to only through
        /// long-polling.
        #[must_use]
        pub fn has_pairing_server(&self) -> bool {
            self.pairing_url.is_some()
        }

        /// The subtypes to register under.
        #[must_use]
        pub fn subtypes(&self) -> Vec<&'static str> {
            self.roles.iter().map(|r| r.service_subtype()).collect()
        }

        /// Long-polling is only ever offered by a LAN endpoint.
        #[must_use]
        pub fn is_consistent_with(&self, deployment: Deployment) -> bool {
            self.longpolling_url.is_none() || deployment == Deployment::Lan
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> PairingToken {
        PairingToken::parse("A1b2C3", TokenKind::Static).expect("a legal token")
    }

    #[test]
    fn an_alias_is_validated_however_the_body_was_parsed() {
        // `requestPairing` needs no bearer, so `nodeIdAlias` is a string an
        // unauthenticated peer chooses: the validation has to be on the way *in*.
        assert!(serde_json::from_str::<NodeIdAlias>(r#""evse7""#).is_ok());
        assert!(serde_json::from_str::<NodeIdAlias>(r#""evse 7""#).is_err());
        assert!(serde_json::from_str::<NodeIdAlias>(r#""""#).is_err());

        // And the deserializer must not require a *borrowed* string. A `&str` here works
        // straight off the wire and then refuses every body something re-parsed — a
        // proxy, a middleware, a fixture — with "expected a borrowed string", which reads
        // as a malformed alias rather than as a deserializer that cannot borrow.
        assert!(serde_json::from_str::<NodeIdAlias>(r#""evse\u0037""#).is_ok());
        assert_eq!(
            serde_json::from_value::<NodeIdAlias>(serde_json::json!("evse7")).ok(),
            Some(NodeIdAlias("evse7".into()))
        );
    }

    #[test]
    fn a_pairing_code_splits_at_the_first_dash() {
        let code = PairingCode::parse("evse7-A1b2C3", TokenKind::Static).unwrap();
        assert_eq!(code.alias.unwrap().0, "evse7");
        assert_eq!(code.token.as_bytes(), b"A1b2C3");

        let code = PairingCode::parse("A1b2C3", TokenKind::Static).unwrap();
        assert!(code.alias.is_none());
    }

    #[test]
    fn a_pairing_code_is_checked_before_the_network_is() {
        // Too short for its kind.
        assert!(PairingCode::parse("abc", TokenKind::Dynamic).is_err());
        assert!(PairingCode::parse("abcd", TokenKind::Dynamic).is_ok());
        assert!(PairingCode::parse("abcde", TokenKind::Static).is_err());
        assert!(PairingCode::parse("abcdef", TokenKind::Static).is_ok());
        // Illegal characters.
        assert!(PairingCode::parse("ab cd", TokenKind::Dynamic).is_err());
        assert!(PairingCode::parse("ab.cd", TokenKind::Dynamic).is_err());
        assert!(PairingCode::parse("", TokenKind::Dynamic).is_err());
        // An empty alias.
        assert!(PairingCode::parse("-abcd", TokenKind::Dynamic).is_err());
    }

    #[test]
    fn everything_an_unauthenticated_peer_chooses_is_bounded() {
        use alloc::string::ToString;

        // The alias is a field of `requestPairing`, which carries no bearer.
        assert!(NodeIdAlias::parse(&"a".repeat(NodeIdAlias::MAX_LEN)).is_ok());
        assert!(NodeIdAlias::parse(&"a".repeat(NodeIdAlias::MAX_LEN + 1)).is_err());

        // And the bound holds on the way *in*, which is the only direction that matters:
        // a newtype whose `parse` validates and whose `Deserialize` does not is validated
        // in name only.
        let long = alloc::format!("\"{}\"", "a".repeat(NodeIdAlias::MAX_LEN + 1));
        assert!(serde_json::from_str::<NodeIdAlias>(&long).is_err());
        assert!(serde_json::from_str::<NodeIdAlias>("\"has space\"").is_err());
        assert!(serde_json::from_str::<NodeIdAlias>("\"\"").is_err());
        assert_eq!(
            serde_json::from_str::<NodeIdAlias>("\"hub7\"").unwrap().0,
            "hub7".to_string()
        );

        // The token goes into an HMAC, so an unbounded one is unbounded work.
        let longest = "a".repeat(PairingToken::MAX_LEN);
        assert!(PairingToken::parse(&longest, TokenKind::Static).is_ok());
        assert!(PairingToken::parse(&alloc::format!("{longest}a"), TokenKind::Static).is_err());
        // Which also bounds the pairing code as a whole, both halves of it.
        assert!(PairingCode::parse(&alloc::format!("hub7-{longest}a"), TokenKind::Static).is_err());
    }

    #[test]
    fn a_secret_never_prints_itself() {
        let code = PairingCode::parse("evse7-SuperSecret", TokenKind::Static).unwrap();
        let rendered = alloc::format!("{:?}", code.token);
        assert!(!rendered.contains("SuperSecret"), "{rendered}");
        let access = AccessToken::new("hunter2");
        assert_eq!(alloc::format!("{access:?}"), "AccessToken(<redacted>)");
    }

    #[test]
    fn a_token_needs_the_entropy_the_specification_demands() {
        assert!(AccessToken::from_entropy(&[0u8; 31]).is_err());
        let token = AccessToken::from_entropy(&[0u8; 32]).unwrap();
        // Base64 of 32 bytes is 44 characters.
        assert_eq!(token.as_str().len(), 44);
        assert_eq!(CommunicationToken::lifetime(), Duration::from_secs(30));
    }

    #[test]
    fn the_challenge_binds_to_the_server_it_was_answered_by() {
        let challenge = [9u8; 32];
        let here = HmacBinding::DomainName("pairing.example.com".into());
        let there = HmacBinding::DomainName("pairing.example.com.evil.test".into());

        let ours = ChallengeResponse::compute(&challenge, &token(), &here).unwrap();
        let theirs = ChallengeResponse::compute(&challenge, &token(), &here).unwrap();
        assert!(ours.verify(&theirs.as_base64()));

        // A man in the middle with his own name gets a different answer, which is the
        // whole point of binding it.
        let forged = ChallengeResponse::compute(&challenge, &token(), &there).unwrap();
        assert!(!ours.verify(&forged.as_base64()));

        // So does the wrong token.
        let wrong = PairingToken::parse("Z9y8X7", TokenKind::Static).unwrap();
        let forged = ChallengeResponse::compute(&challenge, &wrong, &here).unwrap();
        assert!(!ours.verify(&forged.as_base64()));
    }

    #[test]
    fn a_short_challenge_is_refused_rather_than_padded() {
        let binding = HmacBinding::DomainName("x.example".into());
        assert!(matches!(
            ChallengeResponse::compute(&[0u8; 31], &token(), &binding),
            Err(ConnectError::WeakSecret { need: 32, .. })
        ));
    }

    #[test]
    fn a_fingerprint_is_read_with_or_without_colons() {
        let colons = "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:\
                      AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";
        let plain = colons.replace(':', "");
        assert_eq!(
            HmacBinding::from_hex_fingerprint(colons).unwrap(),
            HmacBinding::from_hex_fingerprint(&plain).unwrap()
        );
        // And a fingerprint that is not a SHA-256 is refused.
        assert!(HmacBinding::from_hex_fingerprint("AABB").is_err());
        assert!(HmacBinding::from_hex_fingerprint("nonsense").is_err());
    }

    #[test]
    fn verification_refuses_anything_that_is_not_a_response() {
        let ours = ChallengeResponse::compute(
            &[1u8; 32],
            &token(),
            &HmacBinding::DomainName("x.example".into()),
        )
        .unwrap();
        assert!(!ours.verify("not base64 at all !!"));
        assert!(!ours.verify(""));
        // Right encoding, wrong length.
        assert!(!ours.verify(&base64::engine::general_purpose::STANDARD.encode([0u8; 16])));
    }

    #[test]
    fn only_a_cem_and_an_rm_may_pair() {
        assert!(Role::Cem.can_pair_with(Role::Rm));
        assert!(Role::Rm.can_pair_with(Role::Cem));
        assert!(!Role::Cem.can_pair_with(Role::Cem));
        assert!(!Role::Rm.can_pair_with(Role::Rm));
    }

    #[test]
    fn the_communication_server_is_the_one_the_standard_names() {
        use Deployment::{Lan, Wan};
        // "If a connection is set up between a WAN node and a LAN node, the WAN node must
        // act as a communication server."
        assert_eq!(Deployment::communication_server(Wan, Lan), Role::Cem);
        assert_eq!(Deployment::communication_server(Lan, Wan), Role::Rm);
        // "If ... both in WAN, or both in LAN ... the CEM must act as a communication
        // server."
        assert_eq!(Deployment::communication_server(Wan, Wan), Role::Cem);
        assert_eq!(Deployment::communication_server(Lan, Lan), Role::Cem);
    }

    #[test]
    fn the_dns_sd_record_round_trips_and_accepts_both_spellings() {
        use dnssd::ServiceRecord;
        let record = ServiceRecord {
            endpoint_name: Some("EVSE1038".into()),
            endpoint_logo_url: None,
            pairing_url: Some("https://EVSE1038.local:443/pairing/".into()),
            longpolling_url: None,
            roles: alloc::vec![Role::Rm],
        };
        let pairs = record.txt_pairs();
        assert_eq!(pairs[0], ("txtver", "1".to_string()));
        assert_eq!(record.subtypes(), alloc::vec!["_rm"]);

        let borrowed: alloc::vec::Vec<(&str, &str)> =
            pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let back = ServiceRecord::from_txt(borrowed).unwrap();
        assert_eq!(back.pairing_url, record.pairing_url);

        // The specification's own avahi example writes `txtvers`; both are accepted (E7).
        let back =
            ServiceRecord::from_txt([("txtvers", "1"), ("pairingUrl", "https://x.local/pairing/")])
                .unwrap();
        assert!(back.has_pairing_server());

        // A record with neither URL is useless and is refused.
        assert!(ServiceRecord::from_txt([("txtver", "1")]).is_none());
        // As is one from a future version of the specification.
        assert!(ServiceRecord::from_txt([("txtver", "2"), ("pairingUrl", "x")]).is_none());
    }

    #[test]
    fn long_polling_is_a_lan_only_arrangement() {
        use dnssd::ServiceRecord;
        let record = ServiceRecord {
            longpolling_url: Some("https://rm.local/pairing/".into()),
            roles: alloc::vec![Role::Rm],
            ..ServiceRecord::default()
        };
        assert!(record.is_consistent_with(Deployment::Lan));
        assert!(!record.is_consistent_with(Deployment::Wan));
    }
}
