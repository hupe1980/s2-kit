#![no_main]
//! `Id` is a fixed 64-byte buffer with a length, which is exactly the shape that hides an
//! out-of-bounds write. It also parses from the wire on every single message.

use libfuzzer_sys::fuzz_target;
use s2_kit::types::Id;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Ok(id) = Id::parse(text) else {
        return;
    };
    // Parsing is exact: what went in is what comes out, and it parses again.
    assert_eq!(id.as_str(), text);
    assert_eq!(Id::parse(id.as_str()).expect("round trip"), id);
});
