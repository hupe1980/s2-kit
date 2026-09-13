#![no_main]
//! The RFC 3339 parser is hand-written — two hundred lines of arithmetic on fields a peer
//! controls — so it is the single most likely place in the crate for a panic to live.

use libfuzzer_sys::fuzz_target;
use s2_kit::types::Timestamp;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Ok(parsed) = Timestamp::parse(text) else {
        return;
    };
    // Formatting what we parsed must parse back to the same instant. (Not to the same
    // *string*: the input may carry an offset, and we always print UTC.)
    let formatted = parsed.to_string();
    let again = Timestamp::parse(&formatted).expect("our own output always parses");
    assert_eq!(parsed, again, "{text} formatted as {formatted}");
});
