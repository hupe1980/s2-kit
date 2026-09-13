#![no_main]
//! The validator must terminate and never panic, on any message the decoder accepted.
//!
//! It walks arrays the peer sized, indexes by factors the peer chose and interpolates
//! between values the peer supplied — all of which are places an arithmetic panic hides.

use libfuzzer_sys::fuzz_target;
use s2_kit::codec;
use s2_kit::types::common::ReceptionStatusValues;
use s2_kit::validate::{Context, Validate};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Ok(message) = codec::decode(text) else {
        return;
    };

    let report = message.validate(&Context::empty());
    // A report is self-consistent: errors mean a rejection, and no errors means one of
    // the two accepting statuses.
    if report.has_errors() {
        assert_ne!(report.reception_status(), ReceptionStatusValues::Ok);
    } else {
        assert_eq!(report.reception_status(), ReceptionStatusValues::Ok);
    }

    // And the explanation must not panic on anything the validator accepted.
    let _ = s2_kit::model::explain(&message, &Context::empty());
});
