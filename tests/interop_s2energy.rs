//! Interoperability with `s2energy`, the official Rust crate, at the level of the wire.
//!
//! [`tests/connect_e2e.rs`](connect_e2e) has s2-kit on both ends of a socket, which proves
//! self-consistency. `cargo xtask interop` puts a real peer process on the other end, which
//! proves *behaviour* — and found three defects in the FlexiblePower example
//! implementations doing it. This file is the third thing, and the cheapest: it links the
//! official crate directly and asks whether the two models agree about the **bytes**.
//!
//! That is a different question from either of the others, and it is the one a differential
//! test answers well. Two independent readings of one schema — one hand-written, one
//! `typify`-generated — either produce the same JSON or they do not, and for all 36
//! messages with every optional field set, not for the handful a battery simulator happens
//! to send.
//!
//! No container, no subprocess, no clone: a dev-dependency and `cargo test`.
//!
//! # What it finds
//!
//! `s2energy` 0.3.0 models `ID` as a `Uuid`. `S2J schemas/ID` defines it as the pattern
//! `[a-zA-Z0-9\-_:]{2,64}`, and the standard's own worked examples use `actuator1`, `om1`
//! and `pv1`. So the official crate cannot read **25 of the 43 messages in the standard's
//! own published walkthroughs** — and that is asserted here as a number, because erratum
//! E1 deserves better than a paragraph.
//!
//! # What it cannot reach: `v1.0.0`
//!
//! Only the `0.0.2-beta` profile is cross-checked here, and that is not a choice. The
//! official crate is generated from the beta schema — it requires
//! `DDBC.SystemDescription.present_demand_rate`, which `v1.0.0` removed, and has no
//! `DDBC.PresentDemandStatus`, which `v1.0.0` added — and it is the only other Rust
//! implementation there is. **Nothing implements `v1.0.0`.** So for the two messages the
//! two tags disagree about there is no second reading to differ from, and a differential
//! test cannot be written at all.
//!
//! What those two *are* checked against is the official JSON schema, by a real JSON Schema
//! validator, in `tests/model_matches_schema.rs` — which is the only other authority that
//! exists for them. [`the_v1_shapes_have_no_second_implementation_to_check`] says so in
//! code, so the gap is a recorded fact rather than something a reader has to notice.

#![cfg(all(feature = "testing", feature = "uuid"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use s2_kit::prelude::*;
use s2_kit::schema;
use s2_kit::testing::every_message;
use serde_json::Value;

/// Rewrite every S2 identifier in a message to a UUID, consistently.
///
/// The point is to take `ID`'s *shape* out of the comparison so that everything else —
/// field names, enum spellings, number formats, timestamps, the `Id` capitalisation in
/// `DDBC.OperationMode`, the `supported_commodites` typo — is what the two crates are
/// being judged on. Erratum E1 is tested separately and deliberately; it must not drown
/// out every other difference.
///
/// Which properties are identifiers comes from the **generated** schema table
/// ([`schema::Kind::is_id`]), not from a list of forty-odd field names somebody would have
/// to keep in step with the standard.
fn uuidify(value: &mut Value, type_name: &str, names: &mut BTreeMap<String, String>) {
    let Some(spec) = schema::type_spec(type_name) else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for property in spec.properties {
        let Some(child) = object.get_mut(property.name) else {
            continue;
        };
        match property.kind {
            schema::Kind::Id => {
                if let Some(text) = child.as_str() {
                    let next = names.len();
                    let replacement = names
                        .entry(text.to_owned())
                        .or_insert_with(|| stable_uuid(next))
                        .clone();
                    *child = Value::String(replacement);
                }
            }
            schema::Kind::IdArray(_) => {
                if let Some(items) = child.as_array_mut() {
                    for item in items {
                        if let Some(text) = item.as_str() {
                            let next = names.len();
                            let replacement = names
                                .entry(text.to_owned())
                                .or_insert_with(|| stable_uuid(next))
                                .clone();
                            *item = Value::String(replacement);
                        }
                    }
                }
            }
            schema::Kind::Object(inner) => uuidify(child, inner, names),
            schema::Kind::ObjectArray(inner, _) => {
                if let Some(items) = child.as_array_mut() {
                    for item in items {
                        uuidify(item, inner, names);
                    }
                }
            }
            schema::Kind::Scalar | schema::Kind::ScalarArray(_) => {}
        }
    }
}

/// A UUID that is a function of a counter, so a failure is reproducible.
fn stable_uuid(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// Every message this crate can produce **in `profile`**, with identifiers rewritten to
/// UUIDs.
///
/// The profile matters, and finding out *why* is one of the things this file is for:
/// `s2energy` 0.3.0 is generated from the `0.0.2-beta` schema. It requires
/// `DDBC.SystemDescription.present_demand_rate`, which `v1.0.0` removed, and it has no
/// `DDBC.PresentDemandStatus` variant, which `v1.0.0` added. So "does the official crate
/// read what we write?" has two different answers depending on which profile we wrote,
/// and both are worth asserting.
fn corpus(profile: WireProfile) -> Vec<(MessageKind, Value)> {
    every_message()
        .into_iter()
        .filter(|message| message.kind().exists_in(profile))
        .map(|mut message| {
            // The one field the two tags disagree about. `every_message` builds the
            // v1.0.0 shape; the beta one requires it.
            if let Message::DdbcSystemDescription(d) = &mut message
                && profile.ddbc_system_description_carries_demand_rate()
            {
                d.present_demand_rate = Some(NumberRange::new(0.0, 6000.0));
            }
            let kind = message.kind();
            let mut value: Value = serde_json::from_str(&encode(&message)).unwrap();
            let mut names = BTreeMap::new();
            uuidify(&mut value, kind.as_str(), &mut names);
            (kind, value)
        })
        .collect()
}

#[test]
fn the_official_crate_reads_every_message_this_one_writes() {
    // The direction that matters for a Resource Manager built here talking to a CEM built
    // there: does what we put on the wire parse at all? All 35 messages of the profile
    // the official crate actually speaks, with every optional field set.
    let corpus = corpus(WireProfile::V0_0_2Beta);
    assert_eq!(corpus.len(), 35, "every beta message is in the corpus");

    let mut refused = Vec::new();
    for (kind, value) in corpus {
        let text = serde_json::to_string(&value).unwrap();
        if let Err(e) = serde_json::from_str::<s2energy::common::Message>(&text) {
            refused.push(format!("{kind}: {e}"));
        }
    }
    assert!(
        refused.is_empty(),
        "s2energy could not read what s2-kit wrote:\n  {}",
        refused.join("\n  ")
    );
}

#[test]
fn the_v1_shapes_have_no_second_implementation_to_check() {
    // The honest boundary of this file, asserted rather than assumed.
    //
    // Exactly two messages differ between the two tagged versions, and neither has an
    // independent implementation to be differentially tested against — so their only
    // check is the official JSON schema, which `tests/model_matches_schema.rs` runs a real
    // validator over. If this ever shrinks to one message or to none, a second
    // implementation of `v1.0.0` has appeared and this file has work to do.
    let beta: Vec<MessageKind> = corpus(WireProfile::V0_0_2Beta)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    let v1: Vec<MessageKind> = corpus(WireProfile::V1_0_0)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();

    assert_eq!(v1.len(), 36, "v1.0.0 has all 36 messages");
    assert_eq!(
        beta.len(),
        35,
        "0.0.2-beta has all but DDBC.PresentDemandStatus"
    );

    let only_in_v1: Vec<MessageKind> = v1
        .iter()
        .filter(|kind| !beta.contains(kind))
        .copied()
        .collect();
    assert_eq!(only_in_v1, vec![MessageKind::DdbcPresentDemandStatus]);

    // And the shape difference on the message both profiles have.
    assert!(
        WireProfile::V0_0_2Beta.ddbc_system_description_carries_demand_rate()
            && !WireProfile::V1_0_0.ddbc_system_description_carries_demand_rate(),
        "the other half of the difference"
    );
}

#[test]
fn the_official_crate_speaks_only_the_beta_profile() {
    // Found by this test, and it is the evidence behind D5. `s2energy` 0.3.0 is generated
    // from the `0.0.2-beta` schema:
    //
    //   * it *requires* `DDBC.SystemDescription.present_demand_rate`, which `v1.0.0`
    //     removed;
    //   * it has no `DDBC.PresentDemandStatus` variant, which `v1.0.0` added.
    //
    // Which is to say: the only other Rust implementation cannot speak `v1.0.0` DDBC at
    // all. A crate that supported only the tagged version would have nothing to talk to,
    // and this is that claim with a test under it rather than a paragraph.
    let v1: BTreeMap<MessageKind, Value> = corpus(WireProfile::V1_0_0).into_iter().collect();

    let ddbc = &v1[&MessageKind::DdbcSystemDescription];
    assert!(
        ddbc.get("present_demand_rate").is_none(),
        "v1.0.0 removed the field"
    );
    let e = serde_json::from_str::<s2energy::common::Message>(&ddbc.to_string())
        .expect_err("s2energy requires the beta field");
    assert!(
        e.to_string().contains("present_demand_rate"),
        "unexpected reason: {e}"
    );

    let present = &v1[&MessageKind::DdbcPresentDemandStatus];
    let e = serde_json::from_str::<s2energy::common::Message>(&present.to_string())
        .expect_err("s2energy has no such message");
    assert!(
        e.to_string().contains("unknown variant"),
        "unexpected reason: {e}"
    );

    // Every *other* message is identical between the two tags, so the profile costs
    // exactly these two and nothing else.
    let mismatches: Vec<String> = v1
        .iter()
        .filter(|(kind, _)| {
            !matches!(
                **kind,
                MessageKind::DdbcSystemDescription | MessageKind::DdbcPresentDemandStatus
            )
        })
        .filter(|(_, value)| {
            serde_json::from_str::<s2energy::common::Message>(&value.to_string()).is_err()
        })
        .map(|(kind, _)| kind.to_string())
        .collect();
    assert!(
        mismatches.is_empty(),
        "the two tags differ only in DDBC, so nothing else should fail: {mismatches:?}"
    );
}

#[test]
fn this_crate_reads_back_what_the_official_crate_re_encodes() {
    // The other direction, and the one that catches a *silent* disagreement: s2energy
    // parses our bytes, writes them out again from its own model, and we must still
    // recognise the result as the same message. A field either crate quietly drops shows
    // up here and nowhere else.
    let mut problems = Vec::new();
    for (kind, value) in corpus(WireProfile::V0_0_2Beta) {
        let text = serde_json::to_string(&value).unwrap();
        let Ok(theirs) = serde_json::from_str::<s2energy::common::Message>(&text) else {
            continue; // covered by the test above
        };
        let round_tripped = serde_json::to_string(&theirs).unwrap();
        // Against the profile the official crate actually writes, which is the beta one.
        let options = s2_kit::DecodeOptions::default().profile(WireProfile::V0_0_2Beta);
        match s2_kit::decode_with(&round_tripped, &options).map(|d| d.message) {
            Err(e) => problems.push(format!("{kind}: s2-kit could not read it back: {e}")),
            Ok(ours) => {
                // Compare as JSON: the two crates order fields differently, and field
                // order is not part of the wire contract.
                let ours: Value = serde_json::from_str(&encode(&ours)).unwrap();
                let theirs: Value = serde_json::from_str(&round_tripped).unwrap();
                if ours != theirs {
                    problems.push(format!("{kind}: {ours} != {theirs}"));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the two models disagree about the wire:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn the_official_crate_cannot_read_the_standards_own_examples() {
    // Erratum E1, as a number rather than a paragraph.
    //
    // `S2J schemas/ID` is the pattern `[a-zA-Z0-9\-_:]{2,64}`, in both tagged versions.
    // Its *description* says "An identifier expressed as a UUID", which is not what the
    // pattern says and not what the standard's own walkthroughs use — they identify
    // actuators as `actuator1` and operation modes as `om1`. `s2energy` 0.3.0 changed its
    // internal representation to a `Uuid`, so it refuses them.
    //
    // This is the single clearest reason this crate exists, and it is asserted against
    // the *published examples of the standard itself*, not against an opinion.
    let dir = std::path::Path::new("tests/fixtures/official");
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("the official fixtures")
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut refused = Vec::new();
    for entry in &entries {
        let text = std::fs::read_to_string(entry.path()).unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Whatever else is true, *this* crate reads all of them.
        assert!(decode(&text).is_ok(), "s2-kit refused {name}");
        if let Err(e) = serde_json::from_str::<s2energy::common::Message>(&text) {
            assert!(
                e.to_string().contains("UUID parsing failed"),
                "{name} was refused for a reason other than E1: {e}"
            );
            refused.push(name);
        }
    }

    assert_eq!(entries.len(), 43, "the walkthroughs are 43 messages");
    assert_eq!(
        refused.len(),
        25,
        "s2energy refused {} of the standard's own examples, not 25 — if it is now fewer, \
         E1 may have been fixed upstream, which is good news and a number to update. \
         Refused: {refused:?}",
        refused.len()
    );
}

#[test]
fn the_two_crates_agree_about_the_warts() {
    // The three places the wire is misspelled or oddly cased are the places a
    // hand-written model and a generated one are most likely to part company, and the
    // only ones where being *wrong* is invisible until a peer refuses you.
    let (_, ddbc) = corpus(WireProfile::V0_0_2Beta)
        .into_iter()
        .find(|(kind, _)| *kind == MessageKind::DdbcSystemDescription)
        .expect("a DDBC.SystemDescription");
    let actuator = &ddbc["actuators"][0];
    assert!(
        actuator.get("supported_commodites").is_some(),
        "the typo is the wire name (E5): {actuator}"
    );
    assert!(
        actuator["operation_modes"][0].get("Id").is_some(),
        "DDBC.OperationMode capitalises its id field (E4): {actuator}"
    );

    let text = serde_json::to_string(&ddbc).unwrap();
    serde_json::from_str::<s2energy::common::Message>(&text)
        .expect("and the official crate spells them the same way");

    // `NOT_CONTROLABLE` has one `l` on the wire (E15), and both crates must keep it.
    let ours = encode(&Message::from(SelectControlType {
        message_id: Id::generate(),
        control_type: ControlType::NotControllable,
    }));
    assert!(ours.contains("NOT_CONTROLABLE"), "{ours}");
    serde_json::from_str::<s2energy::common::Message>(&ours)
        .expect("the official crate reads our NOT_CONTROLABLE");
}
