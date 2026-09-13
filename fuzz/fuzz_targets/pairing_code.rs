#![no_main]
//! The pairing code a user types, and the challenge–response it feeds.
//!
//! `PairingCode::parse` applies the minimum length that is all that stands between a
//! device and an offline brute force (erratum E18); `HmacBinding::from_hex_fingerprint`
//! decodes a fingerprint that arrives before any trust exists. Both are hand-written and
//! both run on input somebody else chose.

use libfuzzer_sys::fuzz_target;
use s2_kit::connect::proto::{ChallengeResponse, HmacBinding, PairingCode, PairingToken, TokenKind};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };

    for kind in [TokenKind::Static, TokenKind::Dynamic] {
        if let Ok(code) = PairingCode::parse(text, kind) {
            // Whatever was accepted meets the floor its kind sets, and the alias — which
            // is not secret and is sent as-is — never swallowed the separator.
            let minimum = match kind {
                TokenKind::Static => PairingToken::MIN_STATIC,
                TokenKind::Dynamic => PairingToken::MIN_DYNAMIC,
            };
            assert!(code.token.as_bytes().len() >= minimum);
            assert!(code.token.as_bytes().iter().all(u8::is_ascii_alphanumeric));
            if let Some(alias) = &code.alias {
                assert!(!alias.0.as_str().contains('-'));
            }

            // The response is deterministic in its three inputs and never panics on any
            // of them.
            let binding = HmacBinding::DomainName(text.into());
            if let Ok(response) = ChallengeResponse::compute(&[7u8; 32], &code.token, &binding) {
                let encoded = response.as_base64();
                assert!(response.verify(&encoded));
                assert_eq!(
                    ChallengeResponse::from_base64(&encoded).expect("what we wrote"),
                    response
                );
            }
        }
    }

    // A fingerprint is hexadecimal with optional separators, and anything else must be
    // refused rather than half-decoded.
    if let Ok(binding) = HmacBinding::from_hex_fingerprint(text) {
        match binding {
            HmacBinding::LeafFingerprint(bytes) => assert!(bytes.len() >= 32),
            HmacBinding::DomainName(_) => unreachable!("a fingerprint is not a domain"),
        }
    }

    // And a response off the wire is either 32 bytes or rejected.
    let _ = ChallengeResponse::from_base64(text);
});
