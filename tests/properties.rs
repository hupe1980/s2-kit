//! The laws this crate's types obey, over inputs nobody would have written down.
//!
//! The tests elsewhere assert what the standard's documents say; these assert what must
//! hold for *every* input. Three kinds:
//!
//! 1. **Totality.** Decoding, validating and feeding a session arbitrary bytes never
//!    panics — an arithmetic overflow in a debug build included.
//! 2. **Round trips.** What this crate writes, it reads back unchanged.
//! 3. **Inverses and orderings.** `factor_of` undoes `at_factor`; comparing timestamps
//!    agrees with comparing the instants they name.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use proptest::prelude::*;

use s2_kit::codec::{DecodeOptions, Strictness, decode, decode_with, encode};
use s2_kit::model;
use s2_kit::prelude::*;
use s2_kit::session::{CemConfig, CemSession, RmConfig, RmSession};
use s2_kit::validate::{Context, Validate};

/// Years 1..=9999, which is the whole range RFC 3339 can spell.
///
/// `Timestamp` itself reaches further — a peer may legitimately name a far-future instant
/// — but a value outside this window has no wire form, so a round trip through the wire
/// is not a property it can have.
const MIN_WIRE_SECS: i64 = -62_135_596_800; // 0001-01-01T00:00:00Z
const MAX_WIRE_SECS: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z

prop_compose! {
    fn any_wire_timestamp()(
        secs in MIN_WIRE_SECS..=MAX_WIRE_SECS,
        nanos in 0u32..1_000_000_000,
    ) -> Timestamp {
        Timestamp::from_unix(secs, nanos)
    }
}

prop_compose! {
    /// Any timestamp at all, including the ones no wire form can hold.
    fn any_timestamp()(secs in any::<i64>(), nanos in 0u32..1_000_000_000) -> Timestamp {
        Timestamp::from_unix(secs, nanos)
    }
}

/// A finite `f64` in the range S2 quantities actually occupy, plus the awkward ones.
fn any_finite() -> impl Strategy<Value = f64> {
    prop_oneof![
        9 => -1e9f64..1e9,
        1 => prop_oneof![
            Just(0.0),
            Just(-0.0),
            Just(f64::MIN_POSITIVE),
            Just(f64::MAX),
            Just(f64::MIN),
            // The fill rate `hems` found that does not survive a round trip without
            // `serde_json`'s `float_roundtrip`.
            Just(0.001_319_444_444_444_444_3),
        ],
    ]
}

fn any_id() -> impl Strategy<Value = Id> {
    "[a-zA-Z0-9_:-]{2,64}".prop_map(|s| Id::parse(&s).expect("the strategy is the pattern"))
}

// ---------------------------------------------------------------------------
// 1. Totality
// ---------------------------------------------------------------------------

proptest! {
    /// Decoding arbitrary bytes answers; it never panics and never hangs.
    #[test]
    fn decoding_anything_at_all_is_total(text in ".{0,2000}") {
        let _ = decode(&text);
        let _ = decode_with(&text, &DecodeOptions::default().lenient());
        let _ = s2_kit::codec::peek(&text);
    }

    /// And a session fed arbitrary frames stays a session: it answers or ignores, and it
    /// never panics. This is the one that matters, because it is the code path a hostile
    /// peer reaches.
    #[test]
    fn a_session_fed_anything_at_all_keeps_going(frames in prop::collection::vec(".{0,400}", 1..8)) {
        let now = Timestamp::from_unix(1_700_000_000, 0);
        let mut rm = RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
        let mut cem = CemSession::new(CemConfig::default());
        rm.open(now);
        cem.open(now);
        for frame in &frames {
            let _ = rm.handle_text(frame, now);
            let _ = cem.handle_text(frame, now);
        }
        // Whatever it was handed, it must still be able to say what it wants to send and
        // when it next needs waking.
        while rm.poll_transmit().is_some() {}
        while cem.poll_transmit().is_some() {}
        let _ = rm.poll_timeout();
        let _ = cem.poll_timeout();
    }

    /// An observer fed arbitrary frames stays an observer, and agrees with the engines
    /// about what each one deserved.
    ///
    /// The second half is what makes a replay worth reading: `Observed::status` is what a
    /// receiver *should* answer, and a `RmSession` handed the same frame is what one
    /// actually answers. If those two ever disagree the analyzer is reporting a reading
    /// nobody implements — so they are checked against each other on input nobody wrote.
    #[test]
    fn an_observer_agrees_with_a_session_about_arbitrary_frames(
        frames in prop::collection::vec(".{0,400}", 1..8),
    ) {
        use s2_kit::session::Analyzer;
        use s2_kit::types::common::EnergyManagementRole;

        let now = Timestamp::from_unix(1_700_000_000, 0);
        let mut analyzer = Analyzer::new().strict();
        let mut rm = RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
        rm.open(now);
        // The engine has a `Handshake` waiting; drop it, so both sides start level.
        while rm.poll_transmit().is_some() {}

        for frame in &frames {
            let observed = analyzer.observe(EnergyManagementRole::Cem, frame, now);
            let inbound = rm.handle_text(frame, now);
            while rm.poll_transmit().is_some() {}
            prop_assert_eq!(
                observed.kind, inbound.kind,
                "the two disagree about what {:?} even was", frame
            );
            // A `ReceptionStatus` is the one message a session never answers, so it has
            // no status to compare; the observer still reports on it.
            if observed.kind == Some(s2_kit::MessageKind::ReceptionStatus) {
                continue;
            }
            prop_assert_eq!(
                observed.status, inbound.status,
                "observer says {:?}, session says {:?}, for {:?}",
                observed.status, inbound.status, frame
            );
        }
    }

    /// Replaying an arbitrary transcript is total, and never claims a clean conversation
    /// it also found errors in.
    #[test]
    fn replaying_anything_at_all_is_total(lines in prop::collection::vec(".{0,300}", 0..8)) {
        let text = lines.join("\n");
        let entries = s2_kit::testing::read_s2log(&text);
        let report = s2_kit::testing::replay(&entries);
        prop_assert_eq!(report.lines, entries.len());
        prop_assert!(report.answered <= entries.len());
        prop_assert_eq!(
            report.is_conforming(),
            !report.findings.iter().any(s2_kit::testing::Finding::is_error)
        );
        // The findings are in transcript order, and every one names a line that exists.
        let mut previous = 0;
        for finding in &report.findings {
            prop_assert!(finding.line() >= previous);
            prop_assert!(finding.line() >= 1 && finding.line() <= entries.len());
            previous = finding.line();
        }
    }

    /// Timestamp arithmetic is checked everywhere, at every magnitude.
    #[test]
    fn timestamp_arithmetic_never_overflows(
        a in any_timestamp(),
        b in any_timestamp(),
        millis in any::<u64>(),
    ) {
        let d = Duration::from_millis(millis);
        let _ = a.checked_add(d);
        let _ = a.checked_sub(d);
        let _ = a.checked_duration_since(b);
        let _ = b.checked_duration_since(a);
        prop_assert_eq!(a.saturating_duration_since(b).is_zero(), a <= b || a.checked_duration_since(b).is_none());
        let _ = a.to_civil_utc();
        let _ = a.unix_millis();
    }

    /// Validating an arbitrary well-formed message is total, and its verdict is
    /// self-consistent: `INVALID_CONTENT` exactly when there is an error to report.
    #[test]
    fn validation_is_total_and_self_consistent(
        id in any_id(),
        level in any_finite(),
        factor in -2.0f64..3.0,
    ) {
        let status = Message::from(frbc::StorageStatus {
            message_id: id,
            present_fill_level: level,
        });
        let report = status.validate(&Context::empty());
        prop_assert_eq!(
            report.reception_status() == ReceptionStatusValues::InvalidContent,
            report.has_errors()
        );

        let instruction = Message::from(frbc::Instruction {
            message_id: id,
            id,
            actuator_id: id,
            operation_mode: id,
            operation_mode_factor: factor,
            execution_time: Timestamp::UNIX_EPOCH,
            abnormal_condition: false,
        });
        let report = instruction.validate(&Context::empty());
        // A factor outside [0, 1] is the one thing an empty context can judge here.
        prop_assert_eq!(report.has_errors(), !(0.0..=1.0).contains(&factor));
        prop_assert_eq!(report.has_errors(), report.diagnostic_label().is_some());
    }
}

// ---------------------------------------------------------------------------
// 2. Round trips
// ---------------------------------------------------------------------------

proptest! {
    /// Every timestamp with a wire form survives being written and read back, to the
    /// nanosecond. This is what makes a proxy's re-encoding of an unchanged message a
    /// no-op.
    #[test]
    fn a_timestamp_survives_the_wire(t in any_wire_timestamp()) {
        let written = t.to_string();
        let read: Timestamp = written.parse().expect("what we wrote, we can read");
        prop_assert_eq!(read, t, "{}", written);
        prop_assert!(written.ends_with('Z'));
    }

    /// Offsets are normalised, not remembered: two spellings of one instant read equal.
    #[test]
    fn an_offset_names_the_same_instant_as_its_utc_spelling(
        secs in (MIN_WIRE_SECS + 86_400)..=(MAX_WIRE_SECS - 86_400),
        offset_minutes in -(23i32 * 60 + 59)..=(23i32 * 60 + 59),
    ) {
        let t = Timestamp::from_unix(secs, 0);
        let shifted = secs + i64::from(offset_minutes) * 60;
        let (y, mo, d, h, mi, s) = Timestamp::from_unix(shifted, 0).to_civil_utc();
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let (oh, om) = (offset_minutes.abs() / 60, offset_minutes.abs() % 60);
        let spelled = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{sign}{oh:02}:{om:02}");
        let parsed: Timestamp = spelled.parse().expect("a legal RFC 3339 offset form");
        prop_assert_eq!(parsed, t, "{}", spelled);
    }

    /// A duration is whole milliseconds on the wire, and comes back as the same number.
    #[test]
    fn a_duration_survives_the_wire(millis in any::<u64>()) {
        let d = Duration::from_millis(millis);
        let json = serde_json::to_string(&d).unwrap();
        prop_assert_eq!(json, millis.to_string());
        prop_assert_eq!(serde_json::from_str::<Duration>(&millis.to_string()).unwrap(), d);
    }

    /// An identifier is a bare string on the wire, unchanged in both directions.
    #[test]
    fn an_identifier_survives_the_wire(id in any_id()) {
        let json = serde_json::to_string(&id).unwrap();
        prop_assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
        prop_assert_eq!(id.len(), id.as_str().len());
        prop_assert!((Id::MIN_LEN..=Id::MAX_LEN).contains(&id.len()));
    }

    /// Anything outside the pattern is refused, whatever else it is.
    #[test]
    fn an_identifier_is_exactly_the_pattern(s in ".{0,80}") {
        let legal = (Id::MIN_LEN..=Id::MAX_LEN).contains(&s.len())
            && s.bytes().all(Id::is_legal_byte)
            && s.is_ascii();
        prop_assert_eq!(Id::parse(&s).is_ok(), legal, "{:?}", s);
    }

    /// Encoding a message and decoding it gives the message back — including the small
    /// fill rates that do not survive a round trip without `float_roundtrip`.
    #[test]
    fn a_message_survives_the_wire(
        id in any_id(),
        level in any_finite(),
        value in any_finite(),
    ) {
        for message in [
            Message::from(frbc::StorageStatus { message_id: id, present_fill_level: level }),
            Message::from(PowerMeasurement {
                message_id: id,
                measurement_timestamp: Timestamp::from_unix(1_700_000_000, 123_456_789),
                values: vec![PowerValue::new(CommodityQuantity::ElectricPowerL1, value)],
            }),
        ] {
            let text = encode(&message);
            let back = decode(&text).expect("what we encode, we decode");
            prop_assert_eq!(&back, &message, "{}", text);
            // Canonical: encoding the decoded form reproduces the bytes exactly.
            prop_assert_eq!(encode(&back), text);
        }
    }

    /// Lenient decoding removes exactly the properties the schema does not define, and
    /// removing them twice removes nothing the second time.
    #[test]
    fn pruning_is_idempotent(extras in prop::collection::vec("[a-z]{1,8}", 0..5)) {
        let mut object = serde_json::json!({
            "message_type": "FRBC.StorageStatus",
            "message_id": "m1",
            "present_fill_level": 52.0,
        });
        let expected: std::collections::BTreeSet<String> = extras.iter().cloned().collect();
        for extra in &extras {
            object[extra.as_str()] = serde_json::json!(1);
        }
        let text = object.to_string();

        let options = DecodeOptions {
            strictness: Strictness::Lenient,
            ..DecodeOptions::default()
        };
        let first = decode_with(&text, &options).expect("a lenient decode");
        let pruned: std::collections::BTreeSet<String> = first
            .pruned
            .iter()
            .map(|p| p.trim_start_matches('/').to_string())
            .collect();
        prop_assert_eq!(&pruned, &expected);

        // What came back out has nothing left to prune.
        let again = decode_with(&encode(&first.message), &options).expect("a re-decode");
        prop_assert!(again.pruned.is_empty());
        prop_assert_eq!(again.message, first.message);
    }
}

// ---------------------------------------------------------------------------
// 3. Inverses and orderings
// ---------------------------------------------------------------------------

proptest! {
    /// `factor_of` undoes `at_factor` wherever the range has any width at all.
    ///
    /// This is the arithmetic both roles depend on agreeing about: the CEM picks a factor,
    /// the RM turns it back into watts, and a disagreement here is a device doing
    /// something other than what was asked.
    #[test]
    fn interpolation_and_its_inverse_agree(
        start in -1e6f64..1e6,
        end in -1e6f64..1e6,
        factor in 0.0f64..=1.0,
    ) {
        prop_assume!((end - start).abs() > 1e-3);
        let range = NumberRange::new(start, end);
        let value = range.at_factor(factor);
        let back = range.factor_of(value).expect("a range with width has an inverse");
        prop_assert!((back - factor).abs() < 1e-6, "{} -> {} -> {}", factor, value, back);
        // And the ends are the ends, *exactly*. `start + (end - start) * 1.0` is not
        // always `end` in binary floating point, and both roles compare against the
        // number the description published.
        prop_assert_eq!(range.at_factor(0.0), start);
        prop_assert_eq!(range.at_factor(1.0), end);
        prop_assert_eq!(range.factor_of(start), Some(0.0));
    }

    /// A factor always lands inside the range, whichever way round the range was written.
    /// A discharging battery's fill rate runs from 0 down to a negative number, so "start
    /// is the smaller" is not a property S2 ranges have.
    #[test]
    fn interpolation_stays_within_the_range(
        start in -1e6f64..1e6,
        end in -1e6f64..1e6,
        factor in 0.0f64..=1.0,
    ) {
        let range = NumberRange::new(start, end);
        let (lo, hi) = range.ordered();
        let value = range.at_factor(factor);
        let slack = (hi - lo).abs() * 1e-9 + 1e-9;
        prop_assert!(value >= lo - slack && value <= hi + slack, "{} not in {}..{}", value, lo, hi);
        prop_assert!(range.contains(value.clamp(lo, hi)));
    }

    /// A power range interpolates one value per quantity, in the order it was given.
    #[test]
    fn interpolating_power_answers_once_per_quantity(
        a in -1e5f64..1e5,
        b in -1e5f64..1e5,
        factor in 0.0f64..=1.0,
    ) {
        let ranges = [
            PowerRange::new(a, b, CommodityQuantity::ElectricPowerL1),
            PowerRange::new(b, a, CommodityQuantity::ElectricPowerL2),
        ];
        let values = model::interpolate(&ranges, factor);
        prop_assert_eq!(values.len(), 2);
        prop_assert_eq!(values[0].commodity_quantity, CommodityQuantity::ElectricPowerL1);
        prop_assert_eq!(values[1].commodity_quantity, CommodityQuantity::ElectricPowerL2);
        prop_assert_eq!(
            model::factor_for_power(&ranges, CommodityQuantity::HeatFlowRate, 0.0),
            None,
            "a quantity the ranges do not mention has no factor"
        );
    }

    /// Comparing timestamps agrees with comparing the instants they name.
    #[test]
    fn timestamp_ordering_is_chronological(a in any_timestamp(), b in any_timestamp()) {
        let key = |t: Timestamp| (t.unix_secs(), t.subsec_nanos());
        prop_assert_eq!(a.cmp(&b), key(a).cmp(&key(b)));
        // And a later instant is later by exactly the duration between them.
        if let Some(d) = b.checked_duration_since(a) {
            prop_assert!(b >= a);
            if let Some(forward) = a.checked_add(d) {
                // `Duration` is milliseconds, so the sub-millisecond part is dropped.
                prop_assert!(forward <= b);
                prop_assert!(b.saturating_duration_since(forward).as_millis() < 1);
            }
        }
    }

    /// The whole point of FRBC, as a theorem: a mode that fills raises the level, a mode
    /// that drains lowers it, and leakage and usage only ever subtract.
    #[test]
    fn projecting_a_fill_level_moves_it_the_way_the_rate_says(
        rate in -0.01f64..0.01,
        leakage in 0.0f64..0.001,
        usage in 0.0f64..0.001,
        seconds in 1u64..86_400,
    ) {
        let mode = frbc::OperationMode {
            id: Id::new_const("om1"),
            diagnostic_label: None,
            elements: vec![frbc::OperationModeElement {
                fill_level_range: NumberRange::new(-1e9, 1e9),
                fill_rate: NumberRange::exactly(rate),
                power_ranges: vec![PowerRange::exactly(0.0, CommodityQuantity::ElectricPowerL1)],
                running_costs: None,
            }],
            abnormal_condition_only: false,
        };
        let leak = [frbc::LeakageBehaviourElement {
            fill_level_range: NumberRange::new(-1e9, 1e9),
            leakage_rate: leakage,
        }];
        let d = Duration::from_secs(seconds);
        let from = 50.0;

        let bare = model::project_fill_level(&mode, model::Factor::ZERO, from, d, None, None);
        prop_assert!((bare - (from + rate * d.as_secs_f64())).abs() < 1e-6);

        // Leakage and usage are positive when the level *falls* — the one thing about
        // FRBC that is easiest to get backwards.
        let with_losses = model::project_fill_level(
            &mode, model::Factor::ZERO, from, d, Some(&leak), Some(usage),
        );
        prop_assert!(with_losses <= bare + 1e-9, "losses must not raise the level");

        // A zero-length step moves nothing at all.
        prop_assert_eq!(
            model::project_fill_level(&mode, model::Factor::ZERO, from, Duration::ZERO, Some(&leak), Some(usage)),
            from
        );
    }

    /// A factor is clamped, never invented: `clamping` agrees with `try_new` wherever
    /// `try_new` answers at all.
    #[test]
    fn a_clamped_factor_agrees_where_a_checked_one_exists(raw in -5.0f64..5.0) {
        let clamped = model::Factor::clamping(raw);
        prop_assert!((0.0..=1.0).contains(&clamped.get()));
        match model::Factor::try_new(raw) {
            Some(checked) => prop_assert_eq!(checked.get(), clamped.get()),
            None => prop_assert!(clamped.get() == 0.0 || clamped.get() == 1.0),
        }
    }
}
