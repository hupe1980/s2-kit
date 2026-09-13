//! The TLS half of S2 Connect, which is where its trust model actually lives.
//!
//! # Why this module exists at all
//!
//! In a LAN there is no certificate authority. A heat pump discovered over DNS-SD answers
//! on `https://EVSE1038.local` with a certificate it minted itself, and no amount of
//! public PKI will ever say anything useful about it. S2 Connect's answer is not to
//! weaken TLS but to move the authentication somewhere else: the pairing token, which the
//! end user carried from one device to the other, and which is mixed with the server's
//! **leaf certificate fingerprint** to produce the challenge response
//! (`S2C §Challenge response process`).
//!
//! That inverts the usual order. The certificate is not trusted and then used; it is used
//! and then, if the HMAC comes out right, trusted:
//!
//! > `S2C §4. HTTPS Client checks clientHmacChallengeResponse`: "in case of a local
//! > server, the TLS certificate fingerprint is part of the challenge. So if the challenge
//! > succeeds, the certificate fingerprint is correct, and the certificate can be trusted.
//! > The client **must** pin the self-signed CA (root) certificate, and trust this
//! > certificate for the remainder of the pairing relation."
//!
//! So exactly one operation in this crate talks to a server whose certificate has not been
//! verified — LAN pairing — and it is the operation whose whole purpose is to find out
//! whether that certificate is the right one. Everything afterwards runs against a pinned
//! CA. [`TlsPolicy`] is that distinction made structural, so a driver cannot accidentally
//! carry the pairing posture into a session.
//!
//! # Two fingerprints that are not the same fingerprint
//!
//! The HMAC binds the **leaf**; `ConnectionDetails.certificateFingerprint` pins the **CA**
//! (erratum E19). They are a paragraph apart in the specification and
//! named almost identically. Conflating them yields a pairing that verifies and a
//! connection that does not, so they are different types here: [`Fingerprint`] is carried
//! by [`CapturedIdentity`] for the first and by [`TlsPolicy::PinnedCa`] for the second.

use alloc::string::String;
use alloc::sync::Arc;
// `Vec` comes from the `std` prelude: both features that enable this module imply `std`.

#[cfg(feature = "connect-client")]
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
#[cfg(feature = "connect-client")]
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::CertificateDer;
#[cfg(feature = "connect-client")]
use rustls::pki_types::{ServerName, UnixTime};
#[cfg(feature = "connect-client")]
use rustls::{DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme};

use super::proto::HmacBinding;

/// A SHA-256 certificate fingerprint: the 32 bytes, not the string.
///
/// Kept as bytes because that is what goes into the HMAC. The specification writes them
/// as hexadecimal, usually with colons, and "must be decoded as hexadecimal string before
/// it can be used as input" — a step that is easy to skip and produces a response that is
/// wrong in a way nothing else reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// The fingerprint of a DER-encoded certificate.
    #[must_use]
    pub fn of(der: &CertificateDer<'_>) -> Self {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(der.as_ref());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    /// Parse one written as hexadecimal, with or without colons.
    pub fn parse(hex: &str) -> Result<Self, Error> {
        match HmacBinding::from_hex_fingerprint(hex)? {
            HmacBinding::LeafFingerprint(bytes) => {
                let mut out = [0u8; 32];
                if bytes.len() != 32 {
                    return Err(Error::Fingerprint);
                }
                out.copy_from_slice(&bytes);
                Ok(Self(out))
            }
            HmacBinding::DomainName(_) => Err(Error::Fingerprint),
        }
    }

    /// The bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hexadecimal, colon-separated, which is how every tool prints one.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use core::fmt::Write as _;
        let mut out = String::with_capacity(32 * 3 - 1);
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(':');
            }
            // Writing into a String cannot fail.
            let _ = write!(out, "{b:02X}");
        }
        out
    }

    /// The HMAC binding this fingerprint makes, for a LAN pairing server.
    #[must_use]
    pub fn binding(&self) -> HmacBinding {
        HmacBinding::LeafFingerprint(self.0.to_vec())
    }

    /// Whether two fingerprints match, compared in constant time.
    ///
    /// A fingerprint is not a secret, but comparing it in constant time costs nothing and
    /// removes the question.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq as _;
        self.0.ct_eq(&other.0).into()
    }
}

impl core::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Fingerprint({})", self.to_hex())
    }
}

impl core::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
/// What a handshake revealed about the server.
#[derive(Debug, Clone)]
pub struct CapturedIdentity {
    /// The leaf certificate's fingerprint — the one the HMAC binds to.
    pub leaf: Fingerprint,
    /// The root of the chain the server presented, which is what gets pinned afterwards.
    ///
    /// For the usual LAN arrangement — a self-signed CA and one leaf it signed — this is
    /// the CA. For a server that presented only a leaf, it is that leaf. For one that
    /// sent a leaf and an intermediate but not its root, it is the intermediate: still
    /// sound, since that is what signed the leaf the HMAC verified, but it will need
    /// re-pairing when the intermediate rotates.
    ///
    /// Taking it from **this** chain matters. The specification says to pin "the
    /// self-signed CA (root) certificate" once the challenge succeeds, but the challenge
    /// binds the *leaf* — so a certificate obtained any other way would be pinned on no
    /// evidence at all (erratum E22).
    pub root: CertificateDer<'static>,
    /// The whole chain, leaf first, for a caller that wants to look at it.
    pub chain: Vec<CertificateDer<'static>>,
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
/// How a connection decides whether to trust the server.
#[derive(Debug, Clone)]
pub enum TlsPolicy {
    /// The public PKI, through the **platform's own trust store**. For WAN endpoints.
    ///
    /// Not a bundled root list: a WAN endpoint may well be behind a corporate middlebox
    /// whose CA the operating system trusts and Mozilla's bundle has never heard of, and
    /// an administrator who has distributed a root expects it to be used. Revocation and
    /// local policy come with it for free, which a static list can never provide.
    Web,
    /// Exactly one certificate, and nothing else. For every LAN connection after pairing.
    PinnedCa(CertificateDer<'static>),
    /// **Trust nothing, record everything.** For LAN *pairing* and nothing else.
    ///
    /// The handshake is allowed to succeed against any certificate, because at this point
    /// in the protocol there is nothing to check it against; the challenge response that
    /// follows is what decides whether the server was genuine, and it fails closed. See
    /// the module documentation.
    ///
    /// Using this for anything other than the four pairing requests is a security bug: it
    /// is an unauthenticated channel until the HMAC says otherwise, and the HMAC only says
    /// anything during pairing.
    LanPairingOnly,
}

#[cfg(feature = "connect-client")]
impl TlsPolicy {
    /// Whether this policy actually authenticates the peer.
    #[must_use]
    pub const fn authenticates(&self) -> bool {
        !matches!(self, TlsPolicy::LanPairingOnly)
    }
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
/// A `rustls` client configuration, and the identity the handshake captured.
///
/// The capture is shared with the verifier, so it fills in during the handshake and is
/// readable afterwards without threading anything through the HTTP client.
#[derive(Debug, Clone)]
pub struct TlsClient {
    config: Arc<rustls::ClientConfig>,
    captured: Arc<Capture>,
}

#[cfg(feature = "connect-client")]
impl TlsClient {
    /// Build a client configuration for a policy.
    pub fn new(policy: &TlsPolicy) -> Result<Self, Error> {
        let provider = default_provider()?;
        let captured = Arc::new(Capture::default());

        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?;

        let config = match policy {
            TlsPolicy::Web => {
                let inner = rustls_platform_verifier::Verifier::new(provider)
                    .map_err(|e| Error::Tls(e.to_string()))?;
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(Recording {
                        inner: Verifier::Real(Arc::new(inner)),
                        captured: captured.clone(),
                    }))
                    .with_no_client_auth()
            }
            TlsPolicy::PinnedCa(ca) => {
                let mut roots = RootCertStore::empty();
                roots
                    .add(ca.clone())
                    .map_err(|e| Error::Tls(e.to_string()))?;
                let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider,
                )
                .build()
                .map_err(|e| Error::Tls(e.to_string()))?;
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(Recording {
                        inner: Verifier::Real(inner),
                        captured: captured.clone(),
                    }))
                    .with_no_client_auth()
            }
            TlsPolicy::LanPairingOnly => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Recording {
                    inner: Verifier::AcceptAnything(provider),
                    captured: captured.clone(),
                }))
                .with_no_client_auth(),
        };

        Ok(Self {
            config: Arc::new(config),
            captured,
        })
    }

    /// The configuration, to hand to `reqwest` or `tokio-rustls`.
    #[must_use]
    pub fn config(&self) -> Arc<rustls::ClientConfig> {
        self.config.clone()
    }

    /// What the last handshake revealed, if one has happened.
    #[must_use]
    pub fn captured(&self) -> Option<CapturedIdentity> {
        self.captured.get()
    }

    /// The leaf fingerprint the last handshake presented, as an HMAC binding.
    ///
    /// This is the `F` of `R = HMAC(C, T ‖ F)`, and it is available only after a request
    /// has actually been made — which is why a pairing driver does the first request
    /// before it can check anything.
    #[must_use]
    pub fn binding(&self) -> Option<HmacBinding> {
        self.captured.get().map(|c| c.leaf.binding())
    }

    /// Whether every handshake so far presented the same leaf.
    ///
    /// A LAN pairing runs four requests against a connection pool. If the fingerprint
    /// changes between them, something has moved underneath us and the attempt must not
    /// continue — the HMAC was computed against a certificate that is no longer the one
    /// answering.
    #[must_use]
    pub fn identity_is_stable(&self) -> bool {
        !self.captured.changed()
    }
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
#[derive(Debug, Default)]
struct Capture {
    inner: std::sync::Mutex<Option<CapturedIdentity>>,
    changed: core::sync::atomic::AtomicBool,
}

#[cfg(feature = "connect-client")]
impl Capture {
    fn set(&self, identity: CapturedIdentity) {
        let Ok(mut slot) = self.inner.lock() else {
            // A poisoned lock means another thread panicked mid-capture. Treat the
            // identity as unstable rather than pretending we know it.
            self.changed
                .store(true, core::sync::atomic::Ordering::Relaxed);
            return;
        };
        if let Some(previous) = slot.as_ref()
            && !previous.leaf.matches(&identity.leaf)
        {
            self.changed
                .store(true, core::sync::atomic::Ordering::Relaxed);
        }
        *slot = Some(identity);
    }

    fn get(&self) -> Option<CapturedIdentity> {
        self.inner.lock().ok()?.clone()
    }

    fn changed(&self) -> bool {
        self.changed.load(core::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
#[derive(Debug)]
enum Verifier {
    Real(Arc<dyn ServerCertVerifier>),
    AcceptAnything(Arc<CryptoProvider>),
}

#[cfg(feature = "connect-client")]
#[cfg_attr(docsrs, doc(cfg(feature = "connect-client")))]
/// Wraps a verifier so that the chain is recorded whatever the verdict.
#[derive(Debug)]
struct Recording {
    inner: Verifier,
    captured: Arc<Capture>,
}

#[cfg(feature = "connect-client")]
impl ServerCertVerifier for Recording {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let leaf = end_entity.clone().into_owned();
        let mut chain = alloc::vec![leaf.clone()];
        chain.extend(intermediates.iter().map(|c| c.clone().into_owned()));
        // Record before verifying: a driver that wants to report *which* certificate was
        // refused needs it even when the verdict is no. A server that sent only a leaf
        // roots at that leaf, which is what a self-signed single certificate means.
        self.captured.set(CapturedIdentity {
            leaf: Fingerprint::of(end_entity),
            root: chain.last().cloned().unwrap_or(leaf),
            chain,
        });

        match &self.inner {
            Verifier::Real(inner) => {
                inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            }
            // The whole point: no trust anchor exists yet, and the HMAC that follows is
            // what decides. See `TlsPolicy::LanPairingOnly`.
            Verifier::AcceptAnything(_) => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        match &self.inner {
            Verifier::Real(inner) => inner.verify_tls12_signature(message, cert, dss),
            // Still a real signature check: an attacker must at least hold the key for
            // the certificate he presented, so he cannot replay somebody else's.
            Verifier::AcceptAnything(provider) => verify_tls12_signature(
                message,
                cert,
                dss,
                &provider.signature_verification_algorithms,
            ),
        }
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        match &self.inner {
            Verifier::Real(inner) => inner.verify_tls13_signature(message, cert, dss),
            Verifier::AcceptAnything(provider) => verify_tls13_signature(
                message,
                cert,
                dss,
                &provider.signature_verification_algorithms,
            ),
        }
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        match &self.inner {
            Verifier::Real(inner) => inner.supported_verify_schemes(),
            Verifier::AcceptAnything(provider) => provider
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

/// The crypto provider, installed if the process has not installed one.
///
/// `rustls` requires a process-wide default before any configuration can be built and
/// panics if two are installed. Doing it here, once, keeps that out of every consumer's
/// `main`.
pub fn default_provider() -> Result<Arc<CryptoProvider>, Error> {
    // The application's choice always wins. A library that installs a provider over one
    // the application already chose would give it two crypto stacks and silently use the
    // wrong one; `install_default` also panics on a second call, which is why this is a
    // read first and a write second.
    if let Some(provider) = CryptoProvider::get_default() {
        return Ok(provider.clone());
    }

    // Otherwise fall back to whichever this build bundled. `aws-lc-rs` wins when both are
    // enabled, because a build that went to the trouble of asking for it wanted it.
    #[cfg(feature = "tls-aws-lc-rs")]
    let bundled = Some(rustls::crypto::aws_lc_rs::default_provider());
    #[cfg(all(feature = "tls-ring", not(feature = "tls-aws-lc-rs")))]
    let bundled = Some(rustls::crypto::ring::default_provider());
    #[cfg(not(any(feature = "tls-ring", feature = "tls-aws-lc-rs")))]
    let bundled: Option<CryptoProvider> = None;

    let Some(provider) = bundled else {
        return Err(Error::Tls(
            "no TLS crypto provider: build with the `tls-ring` or `tls-aws-lc-rs` feature, \
             or install one yourself with rustls::crypto::CryptoProvider::install_default"
                .to_string(),
        ));
    };
    // Racing with another thread is fine: whoever wins, we read back the winner.
    let _ = provider.install_default();
    CryptoProvider::get_default()
        .cloned()
        .ok_or_else(|| Error::Tls("no crypto provider could be installed".to_string()))
}

/// A self-signed CA and the leaf it signed: what a LAN endpoint needs to serve HTTPS.
///
/// S2 Connect expects LAN endpoints to mint their own, because there is no authority that
/// could issue a certificate for `EVSE1038.local`. The pairing HMAC is what makes that
/// safe, so generating one is not a shortcut — it is the specified arrangement.
/// Minting a key pair needs a cryptography backend, so this type exists only when one is
/// compiled in — `tls-ring` (the default) or `tls-aws-lc-rs`. An application that installs
/// its own `rustls` provider and brings its own certificates does not need it.
#[cfg(all(
    feature = "connect-server",
    any(feature = "tls-ring", feature = "tls-aws-lc-rs")
))]
#[cfg_attr(
    docsrs,
    doc(cfg(all(
        feature = "connect-server",
        any(feature = "tls-ring", feature = "tls-aws-lc-rs")
    )))
)]
#[derive(Debug)]
pub struct SelfSignedEndpoint {
    /// The CA certificate, which a paired client pins.
    pub ca: CertificateDer<'static>,
    /// The chain to serve: leaf first, then the CA.
    pub chain: Vec<CertificateDer<'static>>,
    /// The leaf's private key.
    pub key: rustls::pki_types::PrivateKeyDer<'static>,
    /// The leaf's fingerprint — the `F` of the challenge response.
    pub leaf: Fingerprint,
}

#[cfg(all(
    feature = "connect-server",
    any(feature = "tls-ring", feature = "tls-aws-lc-rs")
))]
impl SelfSignedEndpoint {
    /// Mint a CA and a leaf valid for the given names.
    ///
    /// Names are whatever a client will dial: `EVSE1038.local`, `127.0.0.1`, an IPv6
    /// literal. A name the certificate does not carry will fail once the client has
    /// pinned the CA, even though it passed during pairing — which is a confusing failure
    /// to debug later, so pass every name the endpoint answers on.
    pub fn generate(names: impl IntoIterator<Item = String>) -> Result<Self, Error> {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose, SanType,
        };

        let names: Vec<String> = names.into_iter().collect();
        let Some(primary) = names.first().cloned() else {
            return Err(Error::Tls(
                "a certificate needs at least one name".to_string(),
            ));
        };

        let ca_key = KeyPair::generate().map_err(|e| Error::Tls(e.to_string()))?;
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca_params.key_usages = alloc::vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "s2-kit LAN endpoint CA");
        let ca_cert = ca_params
            .self_signed(&ca_key)
            .map_err(|e| Error::Tls(e.to_string()))?;

        let leaf_key = KeyPair::generate().map_err(|e| Error::Tls(e.to_string()))?;
        let mut leaf_params = CertificateParams::default();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, primary);
        for name in &names {
            let san = match name.parse::<core::net::IpAddr>() {
                Ok(ip) => SanType::IpAddress(ip),
                Err(_) => SanType::DnsName(
                    name.clone()
                        .try_into()
                        .map_err(|_| Error::Tls(alloc::format!("{name} is not a usable name")))?,
                ),
            };
            leaf_params.subject_alt_names.push(san);
        }
        let issuer = rcgen::Issuer::new(ca_params, ca_key);
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &issuer)
            .map_err(|e| Error::Tls(e.to_string()))?;

        let leaf_der = leaf_cert.der().clone();
        let ca_der = ca_cert.der().clone();
        Ok(Self {
            leaf: Fingerprint::of(&leaf_der),
            chain: alloc::vec![leaf_der, ca_der.clone()],
            ca: ca_der,
            key: rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
        })
    }

    /// A `rustls` server configuration serving this chain.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, Error> {
        let provider = default_provider()?;
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(self.chain.clone(), self.key.clone_key())
            .map_err(|e| Error::Tls(e.to_string()))?;
        Ok(Arc::new(config))
    }
}

/// Something went wrong setting up or inspecting TLS.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The fingerprint is not 32 bytes of hexadecimal.
    #[error("not a SHA-256 certificate fingerprint")]
    Fingerprint,
    /// `rustls` refused the configuration, or no provider could be installed.
    #[error("TLS: {0}")]
    Tls(String),
}

impl From<super::proto::ConnectError> for Error {
    fn from(_: super::proto::ConnectError) -> Self {
        Error::Fingerprint
    }
}

use alloc::string::ToString as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_round_trips_through_the_form_tools_print() {
        let der = CertificateDer::from(alloc::vec![1u8, 2, 3, 4]);
        let fp = Fingerprint::of(&der);
        let text = fp.to_hex();
        assert_eq!(text.len(), 32 * 3 - 1, "32 bytes, colon-separated");
        assert_eq!(Fingerprint::parse(&text).unwrap(), fp);
        // And without the colons, which is the other way tools print them.
        assert_eq!(Fingerprint::parse(&text.replace(':', "")).unwrap(), fp);
        assert!(fp.matches(&Fingerprint::of(&der)));
    }

    #[test]
    fn a_fingerprint_is_the_binding_the_hmac_takes() {
        let der = CertificateDer::from(alloc::vec![9u8; 64]);
        let fp = Fingerprint::of(&der);
        let HmacBinding::LeafFingerprint(bytes) = fp.binding() else {
            panic!("a leaf fingerprint binds as a leaf fingerprint");
        };
        // Not the hex string: "must be decoded as hexadecimal string before it can be
        // used as input".
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..], fp.as_bytes());
    }

    #[test]
    fn something_that_is_not_a_fingerprint_is_refused() {
        assert!(Fingerprint::parse("").is_err());
        assert!(Fingerprint::parse("AA:BB").is_err());
        assert!(Fingerprint::parse("not hexadecimal at all").is_err());
        // The right length of the wrong thing.
        assert!(Fingerprint::parse(&"zz".repeat(32)).is_err());
    }

    #[cfg(feature = "connect-client")]
    #[test]
    fn only_one_policy_declines_to_authenticate_and_it_says_so() {
        assert!(TlsPolicy::Web.authenticates());
        assert!(!TlsPolicy::LanPairingOnly.authenticates());
        let der = CertificateDer::from(alloc::vec![1u8, 2, 3]);
        assert!(TlsPolicy::PinnedCa(der).authenticates());
    }

    #[cfg(feature = "connect-client")]
    #[test]
    fn every_policy_builds_a_usable_configuration() {
        assert!(TlsClient::new(&TlsPolicy::Web).is_ok());
        assert!(TlsClient::new(&TlsPolicy::LanPairingOnly).is_ok());
        // Nothing has been captured before a handshake, which is why a pairing driver
        // must make its first request before it can check anything.
        let client = TlsClient::new(&TlsPolicy::LanPairingOnly).unwrap();
        assert!(client.captured().is_none());
        assert!(client.binding().is_none());
        assert!(client.identity_is_stable());
    }

    #[cfg(feature = "connect-client")]
    #[test]
    fn a_pinned_policy_refuses_something_that_is_not_a_certificate() {
        let nonsense = CertificateDer::from(alloc::vec![0u8; 8]);
        assert!(TlsClient::new(&TlsPolicy::PinnedCa(nonsense)).is_err());
    }

    #[cfg(feature = "connect-client")]
    #[test]
    fn a_changed_leaf_mid_attempt_is_noticed() {
        let capture = Capture::default();
        let one = CertificateDer::from(alloc::vec![1u8; 32]);
        let two = CertificateDer::from(alloc::vec![2u8; 32]);
        let identity = |der: &CertificateDer<'static>| CapturedIdentity {
            leaf: Fingerprint::of(der),
            root: der.clone(),
            chain: alloc::vec![der.clone()],
        };
        capture.set(identity(&one));
        assert!(!capture.changed());
        capture.set(identity(&one));
        assert!(!capture.changed(), "the same certificate twice is fine");
        capture.set(identity(&two));
        assert!(capture.changed(), "a different one is not");
    }

    #[cfg(feature = "connect-server")]
    #[test]
    fn a_lan_endpoint_can_mint_the_certificate_the_specification_expects() {
        let endpoint =
            SelfSignedEndpoint::generate(["EVSE1038.local".to_string(), "127.0.0.1".to_string()])
                .expect("a self-signed CA and leaf");

        // Leaf first, then the CA it was signed by — the order a server sends them.
        assert_eq!(endpoint.chain.len(), 2);
        assert_eq!(endpoint.chain[1], endpoint.ca);
        // The fingerprint the HMAC binds to is the leaf's, not the CA's.
        assert_eq!(endpoint.leaf, Fingerprint::of(&endpoint.chain[0]));
        assert_ne!(endpoint.leaf, Fingerprint::of(&endpoint.ca));
        assert!(endpoint.server_config().is_ok());

        // A client that pins this CA builds.
        #[cfg(feature = "connect-client")]
        assert!(TlsClient::new(&TlsPolicy::PinnedCa(endpoint.ca.clone())).is_ok());
    }

    #[cfg(feature = "connect-server")]
    #[test]
    fn a_certificate_needs_a_name() {
        assert!(SelfSignedEndpoint::generate([]).is_err());
    }
}
