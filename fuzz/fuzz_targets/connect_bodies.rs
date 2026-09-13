#![no_main]
//! The S2 Connect request and response bodies.
//!
//! `requestPairing` needs no bearer, so these are the parsers an unauthenticated peer
//! reaches before any secret has been checked — unlike `codec::decode`, which sits behind
//! an established session.
//!
//! Asserted beyond "does not panic": anything that parses re-serialises, and the result
//! parses back the same. Decoding one way and encoding another is how a proxy and an
//! endpoint disagree about what was said.

use libfuzzer_sys::fuzz_target;
use s2_kit::connect::proto::{
    ConnectionDetails, ConnectionDetailsPost, ConnectionDetailsRequest, FinalizePairing,
    PairingAccepted, PairingRefused, PairingRequest, SessionGrant, SessionRefused,
    SessionRequest, UnpairRequest,
};

/// Parse, re-encode, parse again, and require the two parses to agree.
macro_rules! round_trip {
    ($text:expr, $($ty:ty),* $(,)?) => {
        $(
            if let Ok(parsed) = serde_json::from_str::<$ty>($text) {
                let re_encoded = serde_json::to_string(&parsed)
                    .expect("a body that parsed must be writable");
                let again = serde_json::from_str::<$ty>(&re_encoded)
                    .expect("what we wrote, we read");
                assert_eq!(
                    serde_json::to_string(&again).expect("stable"),
                    re_encoded,
                    "{} did not re-encode stably from {:?}",
                    stringify!($ty),
                    $text
                );
            }
        )*
    };
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };

    round_trip!(
        text,
        // Pairing: the two unauthenticated ones first.
        PairingRequest,
        PairingAccepted,
        PairingRefused,
        ConnectionDetailsRequest,
        ConnectionDetails,
        ConnectionDetailsPost,
        FinalizePairing,
        // Session initiation.
        SessionRequest,
        SessionGrant,
        SessionRefused,
        UnpairRequest,
    );
});
