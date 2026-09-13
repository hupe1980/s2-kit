#![no_main]
//! Anything that decodes must re-encode to something that decodes to the same thing.
//!
//! This is the property that actually matters for a protocol library: a message we
//! understood and forwarded must not change meaning in transit. A proxy that silently
//! rewrites a `NumberRange` is worse than one that refuses the message.

use libfuzzer_sys::fuzz_target;
use s2_kit::codec;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Ok(message) = codec::decode(text) else {
        return;
    };

    let encoded = codec::encode(&message);
    let again = codec::decode(&encoded).expect("our own output always decodes");
    assert_eq!(message, again, "re-encoding changed the message:\n{encoded}");

    // And the pretty form is the same message, not merely similar.
    let pretty = codec::encode_pretty(&message);
    let from_pretty = codec::decode(&pretty).expect("our own output always decodes");
    assert_eq!(message, from_pretty);
});
