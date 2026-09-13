#![no_main]
//! The decoder must never panic on bytes from the network.
//!
//! Everything a peer sends arrives here first, and a peer is not obliged to be friendly:
//! this target feeds it arbitrary bytes and asserts only that it returns rather than
//! aborts, and that its size limit is honoured before any parsing happens.

use libfuzzer_sys::fuzz_target;
use s2_kit::codec::{self, DecodeError, DecodeOptions, Strictness};
use s2_kit::types::WireProfile;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };

    // Every combination of the three knobs, because they take different paths through
    // the schema table.
    for profile in [WireProfile::V1_0_0, WireProfile::V0_0_2Beta] {
        for strictness in [Strictness::Strict, Strictness::Lenient] {
            let mut options = DecodeOptions::default().profile(profile);
            options.strictness = strictness;
            let _ = codec::decode_with(text, &options);
        }
    }
    let _ = codec::peek(text);

    // The size cap is checked before the parser is entered, so a message over it is
    // rejected for being over it and for no other reason.
    let tiny = DecodeOptions::default().max_bytes(64);
    if let Err(DecodeError::TooLarge { len, max }) = codec::decode_with(text, &tiny) {
        assert_eq!(max, 64);
        assert_eq!(len, text.len());
        assert!(len > 64);
    }
});
