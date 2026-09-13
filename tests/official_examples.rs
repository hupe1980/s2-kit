//! The standard's own worked examples, decoded, validated and driven through a session.
//!
//! `tests/fixtures/official/` holds every JSON message from docs
//! `learn/examples/{ev,heat-pump,pv,nocontrol}`, extracted verbatim. They are the only
//! messages in this repository that this crate did not write, which makes them the only
//! ones that can tell it something it does not already believe.
//!
//! A fixture that merely decodes proves the codec and nothing above it, so every one is
//! also **fed to a session** — which is where a message meets the state table, the
//! acknowledgement ledger and the validator.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use s2_kit::prelude::*;
use s2_kit::session::{CemConfig, CemSession, RmConfig, RmSession};
use s2_kit::validate::{Context, Severity, Validate};

fn fixtures() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/official");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("the fixtures directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            Some((name, text))
        })
        .collect();
    out.sort();
    assert!(out.len() >= 40, "the examples went missing: {}", out.len());
    out
}

fn message_type_of(name: &str) -> &str {
    // `ev.FRBC.Instruction.json` -> `FRBC.Instruction`
    name.trim_end_matches(".json")
        .split_once('.')
        .map_or(name, |(_, rest)| rest)
}

#[test]
fn every_documented_example_decodes() {
    for (name, text) in fixtures() {
        let message = decode(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            message.kind().as_str(),
            message_type_of(&name),
            "{name} decoded as the wrong message"
        );
    }
}

#[test]
fn every_documented_example_re_encodes_to_the_same_message() {
    // Not byte-identical — the documentation's formatting is its own — but decoding our
    // own output must give back exactly what we read.
    for (name, text) in fixtures() {
        let message = decode(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ours = encode(&message);
        let again = decode(&ours).unwrap_or_else(|e| panic!("{name} re-encoded: {e}"));
        assert_eq!(again, message, "{name} did not survive a round trip");
    }
}

#[test]
fn no_documented_example_produces_a_validation_error() {
    // The examples are what the standard tells implementers to copy. If a rule of ours
    // fires on one of them as an *error*, the rule is wrong — this test is the guard
    // against a validator that is stricter than the standard.
    let mut complaints: Vec<String> = Vec::new();
    for (name, text) in fixtures() {
        let Ok(message) = decode(&text) else { continue };
        let report = message.validate(&Context::empty());
        for violation in report.violations() {
            if violation.severity == Severity::Error {
                complaints.push(format!("{name}: {violation}"));
            }
        }
    }
    assert!(complaints.is_empty(), "{complaints:#?}");
}

#[test]
fn the_documented_examples_are_warning_free_apart_from_known_spec_defects() {
    // Warnings are allowed to fire — they exist because the standard is unclear — but
    // each one on an official example is worth knowing about, so they are listed.
    let mut warnings: Vec<String> = Vec::new();
    for (name, text) in fixtures() {
        let Ok(message) = decode(&text) else { continue };
        for violation in message.validate(&Context::empty()).warnings() {
            warnings.push(format!("{name}: {}", violation.rule));
        }
    }
    warnings.sort();
    warnings.dedup();
    assert!(
        warnings.is_empty(),
        "the standard's own examples raise warnings: {warnings:#?}"
    );
}

#[test]
fn every_example_is_answered_by_a_session_rather_than_merely_parsed() {
    // A message the parser has met is not a message the
    // engine has answered. Each example is delivered to whichever side should receive
    // it, in a session that has been brought to the right state.
    let now: Timestamp = "2024-08-24T14:15:22Z".parse().expect("a timestamp");
    let mut delivered = 0usize;

    for (name, text) in fixtures() {
        let Ok(message) = decode(&text) else { continue };
        let kind = message.kind();
        let Some(sender) = kind.sender() else {
            continue; // sent by both roles; covered by the conversation tests
        };

        match sender {
            // The Resource Manager sent it, so a manager receives it.
            EnergyManagementRole::Rm => {
                let mut cem =
                    CemSession::new(CemConfig::default().pre_negotiated(WireProfile::V1_0_0));
                cem.open(now);
                // `InstructionStatusUpdate` belongs to no control type but still
                // needs one to be active — there is nothing for it to be about
                // otherwise — so the session is brought to FRBC for those.
                let needs = kind.control_type().or(matches!(
                    kind,
                    MessageKind::InstructionStatusUpdate | MessageKind::RevokeObject
                )
                .then_some(ControlType::FillRateBasedControl));
                if let Some(control_type) = needs {
                    bring_cem_to(&mut cem, control_type, now);
                }
                let inbound = cem.handle_text(&text, now);
                assert!(
                    inbound.accepted(),
                    "{name} was refused by a manager: {}",
                    inbound.report
                );
            }
            // The manager sent it, so a resource receives it.
            EnergyManagementRole::Cem => {
                let mut rm = RmSession::new(
                    RmConfig::default().pre_negotiated(WireProfile::V1_0_0),
                    details_offering(kind.control_type()),
                );
                rm.open(now);
                let inbound = rm.handle_text(&text, now);
                if kind == MessageKind::SelectControlType {
                    assert!(
                        inbound.accepted(),
                        "{name} was refused by a resource: {}",
                        inbound.report
                    );
                } else {
                    // An instruction needs a control type active first; that path is
                    // covered by the conversation tests, so here only the decode and
                    // the state answer are asserted.
                    assert!(inbound.kind.is_some(), "{name} was not even understood");
                }
            }
        }
        delivered += 1;
    }
    assert!(delivered >= 30, "only {delivered} examples were delivered");
}

fn details_offering(control_type: Option<ControlType>) -> ResourceManagerDetails {
    ResourceManagerDetails::builder()
        .resource_id(Id::parse("acme_resource").expect("id"))
        .roles(vec![Role::new(
            RoleType::EnergyStorage,
            Commodity::Electricity,
        )])
        .instruction_processing_delay(Duration::from_millis(500))
        .available_control_types(vec![
            control_type.unwrap_or(ControlType::NotControllable),
            ControlType::FillRateBasedControl,
            ControlType::PowerEnvelopeBasedControl,
            ControlType::NotControllable,
        ])
        .provides_forecast(true)
        .provides_power_measurement_types(vec![
            CommodityQuantity::ElectricPowerL1,
            CommodityQuantity::ElectricPower3PhaseSymmetric,
        ])
        .build()
}

fn bring_cem_to(cem: &mut CemSession, control_type: ControlType, now: Timestamp) {
    // Describe a resource that offers it, then select it and accept the selection.
    let details = details_offering(Some(control_type));
    cem.handle_message(Message::from(details), now);
    let handle = cem
        .select_control_type(control_type, now)
        .expect("selection is always allowed");
    cem.handle_message(Message::from(ReceptionStatus::ok(handle.id)), now);
    while cem.poll_transmit().is_some() {}
    while cem.poll_event().is_some() {}
}

/// The standard's own four conversations, replayed in order through an [`Analyzer`].
///
/// The regression guard the cross-message rules need most: a rule that fires on a message
/// in isolation is caught by the suite above, but one that fires *because of what came
/// before it* can only be wrong here — and these walkthroughs are the one conversation in
/// this repository that this crate did not write.
///
/// Two of the four contradict themselves; the findings below are the record of that (E26).
#[test]
fn an_observer_finds_exactly_the_recorded_defects_in_the_documented_conversations() {
    use s2_kit::session::Analyzer;
    use s2_kit::types::common::EnergyManagementRole;

    // The documented order of each walkthrough. The examples are published as separate
    // files, so the sequence is the documentation's and is written down here rather than
    // inferred — inferring it would make the test agree with whatever the code does.
    let conversations: [(&str, &[&str]); 4] = [
        (
            "ev",
            &[
                "Handshake",
                "HandshakeResponse",
                "ResourceManagerDetails",
                "SelectControlType",
                "FRBC.SystemDescription",
                "FRBC.StorageStatus",
                "FRBC.ActuatorStatus",
                "FRBC.Instruction",
                "InstructionStatusUpdate",
                "PowerMeasurement",
                "SessionRequest",
            ],
        ),
        (
            "heat-pump",
            &[
                "Handshake",
                "HandshakeResponse",
                "ResourceManagerDetails",
                "SelectControlType",
                "FRBC.SystemDescription",
                "FRBC.LeakageBehaviour",
                "FRBC.UsageForecast",
                "FRBC.StorageStatus",
                "FRBC.ActuatorStatus",
                "FRBC.TimerStatus",
                "FRBC.Instruction",
                "InstructionStatusUpdate",
                "PowerMeasurement",
                "SessionRequest",
            ],
        ),
        (
            "pv",
            &[
                "Handshake",
                "HandshakeResponse",
                "ResourceManagerDetails",
                "SelectControlType",
                "PEBC.PowerConstraints",
                "PEBC.EnergyConstraint",
                "PowerForecast",
                "PEBC.Instruction",
                "InstructionStatusUpdate",
                "PowerMeasurement",
                "SessionRequest",
            ],
        ),
        (
            "nocontrol",
            &[
                "Handshake",
                "HandshakeResponse",
                "ResourceManagerDetails",
                "SelectControlType",
                "PowerForecast",
                "PowerMeasurement",
                "SessionRequest",
            ],
        ),
    ];

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/official");
    let now: Timestamp = "2024-08-24T14:15:22Z".parse().expect("a timestamp");
    let mut found: Vec<(&str, &str, &str, String)> = Vec::new();

    for (example, order) in conversations {
        // These walkthroughs negotiate `0.0.2-beta` in their own handshakes, so the
        // observer must read them as that profile — which is the whole point of the
        // profile being a parameter.
        let mut analyzer = Analyzer::new().profile(WireProfile::V0_0_2Beta);
        for kind in order {
            let path = dir.join(format!("{example}.{kind}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let message = decode(&text).unwrap_or_else(|e| panic!("{example}.{kind}: {e}"));

            // Who spoke: the `Handshake` says so itself, `SessionRequest` may come from
            // either and the resource is the one that ends these walkthroughs, and every
            // other message has exactly one sender.
            let sender = match &message {
                Message::Handshake(h) => match h.role {
                    EnergyManagementRole::Cem => EnergyManagementRole::Cem,
                    EnergyManagementRole::Rm => EnergyManagementRole::Rm,
                },
                _ => message.kind().sender().unwrap_or(EnergyManagementRole::Rm),
            };

            let observed = analyzer.observe(sender, &text, now);
            for violation in observed.report.errors() {
                found.push((
                    example,
                    *kind,
                    violation.rule.as_str(),
                    violation.path.clone(),
                ));
            }
        }
    }

    // Every error the standard's own conversations produce, by name (E26): the EV
    // walkthrough's `FRBC.ActuatorStatus` names operation mode `"string"` while its own
    // description declares `om1` and `om2`, and the heat-pump walkthrough declares
    // `actuator1` then addresses `actuator0` and `actuator`, and names instruction
    // `"string"`.
    //
    // Asserted *exactly* rather than allowed, so a new inconsistency in an updated
    // walkthrough fails this test instead of joining a tolerated set.
    let expected: Vec<(&str, &str, &str, String)> = vec![
        (
            "ev",
            "FRBC.ActuatorStatus",
            "S2-FRBC-004",
            "/active_operation_mode_id".to_string(),
        ),
        (
            "heat-pump",
            "FRBC.ActuatorStatus",
            "S2-FRBC-003",
            "/actuator_id".to_string(),
        ),
        (
            "heat-pump",
            "FRBC.TimerStatus",
            "S2-FRBC-003",
            "/actuator_id".to_string(),
        ),
        (
            "heat-pump",
            "FRBC.Instruction",
            "S2-FRBC-003",
            "/actuator_id".to_string(),
        ),
        (
            "heat-pump",
            "InstructionStatusUpdate",
            "S2-INST-002",
            "/instruction_id".to_string(),
        ),
    ];
    assert_eq!(
        found, expected,
        "the standard's walkthroughs produce different findings than E26 records"
    );
}

#[test]
fn the_examples_cover_most_of_the_message_catalogue() {
    // Not all of it — the documentation has no OMBC, DDBC or PPBC walkthrough — and
    // saying which is missing by name is more useful than a percentage.
    let covered: BTreeSet<String> = fixtures()
        .iter()
        .map(|(name, _)| message_type_of(name).to_string())
        .collect();
    let missing: Vec<&str> = MessageKind::ALL
        .iter()
        .map(|k| k.as_str())
        .filter(|k| !covered.contains(*k))
        .collect();

    assert_eq!(
        missing,
        vec![
            "ReceptionStatus",
            "RevokeObject",
            "PPBC.PowerProfileDefinition",
            "PPBC.PowerProfileStatus",
            "PPBC.ScheduleInstruction",
            "PPBC.StartInterruptionInstruction",
            "PPBC.EndInterruptionInstruction",
            "OMBC.SystemDescription",
            "OMBC.Status",
            "OMBC.TimerStatus",
            "OMBC.Instruction",
            "FRBC.FillLevelTargetProfile",
            "DDBC.SystemDescription",
            "DDBC.ActuatorStatus",
            "DDBC.TimerStatus",
            "DDBC.AverageDemandRateForecast",
            "DDBC.PresentDemandStatus",
            "DDBC.Instruction",
        ],
        "the set of messages the standard does not document by example has changed"
    );
}

#[test]
fn the_fixture_directory_matches_what_the_documentation_contains() {
    // A fixture that is deleted, renamed or silently emptied should fail here rather
    // than quietly reduce coverage.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/official");
    let count = std::fs::read_dir(dir).expect("directory").count();
    assert_eq!(count, 43, "the extracted example count changed");
}
