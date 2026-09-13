//! The conversations the standard documents, run end to end between two engines.
//!
//! docs `learn/examples/{ev,heat-pump,pv,nocontrol}` each give a complete message
//! sequence. These tests are those sequences, with a real `CemSession` and a real
//! `RmSession` exchanging real JSON through an in-memory pipe on a virtual clock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use s2_kit::prelude::*;
use s2_kit::session::{CemConfig, CemEvent, Instructed, OutOfOrderPolicy, RmConfig, RmEvent};
use s2_kit::testing::{Conversation, Direction, battery_system, charge, pv_constraints};
use s2_kit::validate::rules;

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

/// `RmToCem` for brevity in the expected flows below.
const UP: Direction = Direction::RmToCem;
const DOWN: Direction = Direction::CemToRm;

#[test]
fn the_documented_frbc_conversation_runs_end_to_end() {
    let mut c = Conversation::battery();
    c.open();

    // The handshake, then the resource describing itself — exactly the order the EV and
    // heat-pump examples show.
    assert_eq!(
        c.flow_without_acks(),
        vec![
            (UP, MessageKind::Handshake),
            (DOWN, MessageKind::Handshake),
            (DOWN, MessageKind::HandshakeResponse),
            (UP, MessageKind::ResourceManagerDetails),
        ]
    );
    assert!(matches!(c.rm.state(), SessionState::Connected));
    assert!(matches!(c.cem.state(), SessionState::Connected));

    // The manager picks the one control type the battery offers.
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    assert!(matches!(
        c.rm.state(),
        SessionState::ControlTypeSelected(ControlType::FillRateBasedControl)
    ));
    assert!(matches!(
        c.cem.state(),
        SessionState::ControlTypeSelected(ControlType::FillRateBasedControl)
    ));

    // Both sides were told, and neither refused anything.
    let rm_events = c.take_rm_events();
    assert!(rm_events.iter().any(|e| matches!(
        e,
        RmEvent::Ready {
            control_type: ControlType::FillRateBasedControl
        }
    )));
    let cem_events = c.take_cem_events();
    assert!(cem_events.iter().any(|e| matches!(
        e,
        CemEvent::Ready {
            control_type: ControlType::FillRateBasedControl
        }
    )));
    assert!(
        cem_events
            .iter()
            .any(|e| matches!(e, CemEvent::ResourceDescribed(_)))
    );

    // The resource describes its abstract device and its state.
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.rm.send(
        frbc::StorageStatus {
            message_id: Id::parse("ss1").unwrap(),
            present_fill_level: 52.0,
        },
        c.now,
    )
    .unwrap();
    c.pump();

    // The manager instructs it to charge at half power, and the resource is handed the
    // instruction already resolved.
    c.cem.instruct(charge("instr0", 0.5, c.now), c.now).unwrap();
    c.pump();

    let Instructed { explanation, .. } = c.first_instruction().expect("an instruction arrived");
    assert_eq!(
        explanation.operation_mode.as_ref().unwrap().1.as_deref(),
        Some("Charging")
    );
    assert_eq!(explanation.electric_power(), Some(2500.0));
    assert_eq!(explanation.fill_rate, Some(0.00125));
    assert!(explanation.is_actionable());

    // The resource says what it did about it. That is the second of the two answers an
    // instruction gets.
    c.rm.instruction_status(
        Id::parse("instr0").unwrap(),
        InstructionStatus::Accepted,
        c.now,
    )
    .unwrap();
    c.pump();

    let cem_events = c.take_cem_events();
    assert!(cem_events.iter().any(|e| matches!(
        e,
        CemEvent::InstructionStatus(u) if u.status_type == InstructionStatus::Accepted
    )));
    c.assert_no_refusals();
}

#[test]
fn the_documented_pebc_conversation_runs_end_to_end() {
    let mut c = Conversation::pv();
    c.open();
    c.cem
        .select_control_type(ControlType::PowerEnvelopeBasedControl, c.now)
        .unwrap();
    c.pump();

    c.rm.send(pv_constraints(c.now), c.now).unwrap();
    c.pump();

    // Curtail to 2 kW for an hour — inside the −4000..0 the inverter allowed.
    let instruction = pebc::Instruction {
        message_id: Id::parse("m-instr").unwrap(),
        id: Id::parse("envelope1").unwrap(),
        execution_time: c.now,
        abnormal_condition: false,
        power_constraints_id: Id::parse("powerConstraint1").unwrap(),
        power_envelopes: vec![pebc::PowerEnvelope {
            id: Id::parse("pe_xxx").unwrap(),
            commodity_quantity: CommodityQuantity::ElectricPowerL1,
            power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                Duration::from_secs(3600),
                -2000.0,
                0.0,
            )],
        }],
    };
    c.cem.instruct(instruction, c.now).unwrap();
    c.pump();

    let instructed = c.first_instruction().expect("an instruction arrived");
    assert_eq!(instructed.explanation.limits.len(), 1);
    assert_eq!(instructed.explanation.limits[0].1.lower, -2000.0);
    c.assert_no_refusals();
}

#[test]
fn an_envelope_outside_what_the_inverter_allowed_is_refused_before_it_is_sent() {
    let mut c = Conversation::pv();
    c.open();
    c.cem
        .select_control_type(ControlType::PowerEnvelopeBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(pv_constraints(c.now), c.now).unwrap();
    c.pump();

    // −6 kW is outside the −4000..0 the inverter published.
    let instruction = pebc::Instruction {
        message_id: Id::parse("m-instr").unwrap(),
        id: Id::parse("envelope2").unwrap(),
        execution_time: c.now,
        abnormal_condition: false,
        power_constraints_id: Id::parse("powerConstraint1").unwrap(),
        power_envelopes: vec![pebc::PowerEnvelope {
            id: Id::parse("pe_yyy").unwrap(),
            commodity_quantity: CommodityQuantity::ElectricPowerL1,
            power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                Duration::from_secs(3600),
                -6000.0,
                0.0,
            )],
        }],
    };
    let error = c.cem.instruct(instruction, c.now).unwrap_err();
    // The manager learns at the call site, not from a rejection a second later.
    let message = error.to_string();
    assert!(
        message.contains(rules::PEBC_OUTSIDE_ALLOWED.as_str()),
        "{message}"
    );
}

#[test]
fn an_instruction_for_an_actuator_that_does_not_exist_is_answered_invalid_content() {
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.pump();

    // Build it by hand so the manager's own outbound validation does not stop it: this
    // is the case where the *peer* is wrong.
    let bogus = r#"{"message_type":"FRBC.Instruction","message_id":"mx","id":"ix",
        "actuator_id":"nope","operation_mode":"charge","operation_mode_factor":0.5,
        "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#;
    let inbound = c.rm.handle_text(bogus, c.now);

    assert_eq!(inbound.status, ReceptionStatusValues::InvalidContent);
    assert!(inbound.report.contains(rules::FRBC_UNKNOWN_ACTUATOR));

    // And the answer carries the rule identifier, so an operator can grep for it.
    let out = c.rm.poll_transmit().unwrap();
    assert!(out.text.contains("S2-FRBC-003"), "{}", out.text);
    assert!(out.text.contains("INVALID_CONTENT"), "{}", out.text);
}

#[test]
fn a_message_that_is_not_allowed_yet_is_refused_but_does_not_close_the_session() {
    let mut c = Conversation::battery();
    c.open();
    // No control type has been selected, so an instruction has no business arriving.
    let early = r#"{"message_type":"FRBC.Instruction","message_id":"mx","id":"ix",
        "actuator_id":"actuator1","operation_mode":"charge","operation_mode_factor":0.5,
        "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#;
    let inbound = c.rm.handle_text(early, c.now);

    assert_eq!(inbound.status, ReceptionStatusValues::InvalidContent);
    assert!(inbound.report.contains(rules::NOT_ALLOWED_IN_STATE));
    assert!(
        matches!(c.rm.state(), SessionState::Connected),
        "a peer that is early is not a peer that is hostile"
    );
}

#[test]
fn strict_ordering_closes_the_session_when_it_is_asked_to() {
    let mut c = Conversation::new(
        RmConfig::default().strict_ordering(),
        s2_kit::testing::battery_details(),
        CemConfig::default(),
    );
    c.open();
    let early = r#"{"message_type":"FRBC.Instruction","message_id":"mx","id":"ix",
        "actuator_id":"actuator1","operation_mode":"charge","operation_mode_factor":0.5,
        "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#;
    c.rm.handle_text(early, c.now);
    assert!(c.rm.state().is_closed());
    assert_eq!(RmConfig::default().out_of_order, OutOfOrderPolicy::Report);
}

#[test]
fn an_unanswered_message_produces_an_event_rather_than_a_stall() {
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();

    // The resource sends a status and the manager never answers.
    let handle =
        c.rm.send(
            frbc::StorageStatus {
                message_id: Id::parse("ss1").unwrap(),
                present_fill_level: 52.0,
            },
            c.now,
        )
        .unwrap();
    c.drop_rm_traffic();

    assert_eq!(c.rm.poll_timeout(), Some(at("2024-01-01T12:00:05Z")));
    c.advance_silently(Duration::from_secs(6));

    let events = c.take_rm_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RmEvent::AckTimedOut(h) if *h == handle)),
        "{events:#?}"
    );
    // And the session is still perfectly usable: a timeout is an event, not a disconnect.
    assert!(!c.rm.state().is_closed());
}

#[test]
fn a_duplicate_delivery_is_answered_again_and_not_processed_twice() {
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.pump();
    let _ = c.take_rm_events();

    let instruction = encode(&Message::from(charge("instr0", 0.5, c.now)));
    let first = c.rm.handle_text(&instruction, c.now);
    let second = c.rm.handle_text(&instruction, c.now);

    assert_eq!(first.status, ReceptionStatusValues::Ok);
    assert_eq!(
        second.status,
        ReceptionStatusValues::Ok,
        "a retried message is answered again"
    );

    // But the instruction was only handed to the application once.
    let instructions = c
        .take_rm_events()
        .into_iter()
        .filter(|e| matches!(e, RmEvent::Instruction(_)))
        .count();
    assert_eq!(instructions, 1);
}

#[test]
fn a_permanent_error_closes_the_session_as_the_standard_says() {
    let mut c = Conversation::battery();
    c.open();
    c.rm.send(
        frbc::StorageStatus {
            message_id: Id::parse("ss1").unwrap(),
            present_fill_level: 52.0,
        },
        c.now,
    )
    .unwrap_err(); // not allowed before a control type is selected

    let handle =
        c.rm.send(
            PowerMeasurement {
                message_id: Id::parse("pm1").unwrap(),
                measurement_timestamp: c.now,
                values: vec![PowerValue::new(
                    CommodityQuantity::ElectricPower3PhaseSymmetric,
                    1000.0,
                )],
            },
            c.now,
        )
        .unwrap();
    c.drop_rm_traffic();

    let nack = encode(&Message::from(ReceptionStatus::error(
        handle.id,
        ReceptionStatusValues::PermanentError,
        "cannot go on",
    )));
    c.rm.handle_text(&nack, c.now);

    assert!(c.rm.state().is_closed());
    let events = c.take_rm_events();
    assert!(events.iter().any(|e| matches!(
        e,
        RmEvent::Closed {
            reason: s2_kit::session::CloseReason::PermanentError(_),
            ..
        }
    )));
}

#[test]
fn under_s2_connect_there_is_no_handshake_at_all() {
    let mut c = Conversation::new(
        RmConfig::default().pre_negotiated(WireProfile::V1_0_0),
        s2_kit::testing::battery_details(),
        CemConfig::default().pre_negotiated(WireProfile::V1_0_0),
    );
    c.open();

    // `S2C §Communication - JSON messages`: the handshake messages "can not be sent".
    assert_eq!(
        c.flow_without_acks(),
        vec![(UP, MessageKind::ResourceManagerDetails)]
    );
    assert!(matches!(c.rm.state(), SessionState::Connected));
    assert!(c.cem.details().is_some());
    c.assert_no_refusals();
}

#[test]
fn a_version_the_resource_never_offered_closes_the_session() {
    let mut c = Conversation::battery();
    c.rm.open(c.now);
    // Drop the handshake on the floor and answer with a version nobody offered.
    c.drop_rm_traffic();
    let response = r#"{"message_type":"HandshakeResponse","message_id":"hr1",
        "selected_protocol_version":"9.9.9"}"#;
    c.rm.handle_text(response, c.now);

    assert!(c.rm.state().is_closed());
    let events = c.take_rm_events();
    assert!(events.iter().any(|e| matches!(
        e,
        RmEvent::Closed {
            reason: s2_kit::session::CloseReason::UnsupportedVersion { .. },
            ..
        }
    )));
}

#[test]
fn the_two_specifications_spell_one_version_two_ways_and_the_handshake_survives_both() {
    // Erratum E25: `S2C §Versioning of JSON Schema files` requires `v1.0.0` while every
    // deployed S2 JSON handshake writes `1.0.0`. A peer that speaks S2 Connect as well
    // may therefore offer the `v` spelling in a plain handshake, and comparing the two
    // with `==` is a session that never opens between implementations that agree.
    let mut c = Conversation::battery();
    c.rm.open(c.now);
    c.drop_rm_traffic();
    let response = r#"{"message_type":"HandshakeResponse","message_id":"hr1",
        "selected_protocol_version":"v1.0.0"}"#;
    c.rm.handle_text(response, c.now);

    assert!(!c.rm.state().is_closed(), "v1.0.0 is 1.0.0");
    assert_eq!(c.rm.profile(), WireProfile::V1_0_0);

    // And the same latitude on the manager's side of the negotiation.
    let mut c = Conversation::battery();
    c.cem.open(c.now);
    c.drop_cem_traffic();
    let handshake = r#"{"message_type":"Handshake","message_id":"h1","role":"RM",
        "supported_protocol_versions":["v0.0.2-beta"]}"#;
    c.cem.handle_text(handshake, c.now);
    assert!(!c.cem.state().is_closed(), "v0.0.2-beta is 0.0.2-beta");
    assert_eq!(c.cem.profile(), WireProfile::V0_0_2Beta);
    // What goes back out is this side's own spelling, never the peer's.
    let answer = core::iter::from_fn(|| c.cem.poll_transmit())
        .map(|out| out.text)
        .find(|text| text.contains("HandshakeResponse"))
        .expect("a handshake response");
    assert!(
        answer.contains(r#""selected_protocol_version":"0.0.2-beta""#),
        "{answer}"
    );

    // The latitude is exactly one leading `v` and nothing else: a version nobody offered
    // still closes the session.
    let mut c = Conversation::battery();
    c.rm.open(c.now);
    c.drop_rm_traffic();
    c.rm.handle_text(
        r#"{"message_type":"HandshakeResponse","message_id":"hr1",
            "selected_protocol_version":"1.0.1"}"#,
        c.now,
    );
    assert!(c.rm.state().is_closed());
}

#[test]
fn the_two_sides_settle_on_the_beta_profile_when_that_is_all_they_share() {
    let mut c = Conversation::new(
        RmConfig::default().offering([ProtocolVersion::V0_0_2_BETA]),
        s2_kit::testing::battery_details(),
        CemConfig::default(),
    );
    c.open();
    assert_eq!(c.rm.profile(), WireProfile::V0_0_2Beta);
    assert_eq!(c.cem.profile(), WireProfile::V0_0_2Beta);

    // And in that profile the message that is new in v1.0.0 cannot be sent.
    let error =
        c.rm.send(
            ddbc::PresentDemandStatus {
                message_id: Id::parse("pd1").unwrap(),
                present_demand_rate: NumberRange::new(0.0, 1.0),
            },
            c.now,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        s2_kit::session::SendError::NotInProfile { .. }
    ));
}

#[test]
fn revoking_a_constraint_takes_it_out_of_the_picture() {
    let mut c = Conversation::pv();
    c.open();
    c.cem
        .select_control_type(ControlType::PowerEnvelopeBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(pv_constraints(c.now), c.now).unwrap();
    c.pump();
    assert_eq!(c.cem.registry().pebc_constraints.len(), 1);

    c.rm.send(
        RevokeObject {
            message_id: Id::parse("rev1").unwrap(),
            object_type: RevokableObjects::PebcPowerConstraints,
            object_id: Id::parse("powerConstraint1").unwrap(),
        },
        c.now,
    )
    .unwrap();
    c.pump();

    assert!(c.cem.registry().pebc_constraints.is_empty());
    let events = c.take_cem_events();
    assert!(events.iter().any(|e| matches!(e, CemEvent::Revoked { .. })));
}

#[test]
fn a_description_published_for_later_becomes_effective_when_its_time_comes() {
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();

    let later = at("2024-01-01T13:00:00Z");
    c.rm.send(battery_system(later), c.now).unwrap();
    c.pump();

    assert!(
        c.cem.registry().frbc.is_none(),
        "not effective until valid_from"
    );
    assert_eq!(c.cem.poll_timeout(), Some(later));

    c.advance(Duration::from_secs(3600));
    assert!(c.cem.registry().frbc.is_some());
    let events = c.take_cem_events();
    assert!(events.iter().any(|e| matches!(
        e,
        CemEvent::DescriptionActivated {
            kind: MessageKind::FrbcSystemDescription
        }
    )));
}

#[test]
fn a_timer_in_the_way_is_reported_rather_than_hidden() {
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(battery_system(c.now), c.now).unwrap();
    // The battery is discharging, and its cooldown timer has not finished.
    c.rm.send(
        frbc::ActuatorStatus {
            message_id: Id::parse("as1").unwrap(),
            actuator_id: Id::parse("actuator1").unwrap(),
            active_operation_mode_id: Id::parse("discharge").unwrap(),
            operation_mode_factor: 1.0,
            previous_operation_mode_id: None,
            transition_timestamp: None,
        },
        c.now,
    )
    .unwrap();
    c.rm.send(
        frbc::TimerStatus {
            message_id: Id::parse("ts1").unwrap(),
            timer_id: Id::parse("cooldown").unwrap(),
            actuator_id: Id::parse("actuator1").unwrap(),
            finished_at: at("2024-01-01T12:10:00Z"),
        },
        c.now,
    )
    .unwrap();
    c.pump();
    let _ = c.take_rm_events();

    // Now ask it to go back to idle, which the cooldown blocks.
    let mut instruction = charge("instr1", 0.0, c.now);
    instruction.operation_mode = Id::parse("idle").unwrap();
    c.cem.instruct(instruction, c.now).unwrap();
    c.pump();

    let instructed = c.first_instruction().unwrap();
    assert!(!instructed.explanation.is_actionable());
    assert_eq!(
        instructed.explanation.blocked_by[0].1.as_deref(),
        Some("Minimum discharge time")
    );

    // It is a warning, not a refusal: the resource decides, not the protocol.
    let events = c.take_rm_events();
    assert!(events.iter().any(|e| matches!(
        e,
        RmEvent::Warnings { report, .. } if report.contains(rules::BLOCKED_BY_TIMER)
    )));
}

#[test]
fn a_session_request_ends_both_sides() {
    let mut c = Conversation::battery();
    c.open();
    c.rm.request_session(
        SessionRequestType::Terminate,
        Some("shutting down".into()),
        c.now,
    )
    .unwrap();
    c.pump();

    assert!(c.rm.state().is_closed());
    assert!(c.cem.state().is_closed());
    let events = c.take_cem_events();
    assert!(events.iter().any(|e| matches!(
        e,
        CemEvent::SessionRequested {
            request: SessionRequestType::Terminate,
            ..
        }
    )));
}

#[test]
fn a_transcript_reads_back_and_replays_through_an_analyzer() {
    // The point of the format: a bug report is a transcript, and a transcript that cannot
    // be read back is a bug report nobody can run.
    use s2_kit::session::Analyzer;

    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();

    let log = c.to_s2log();
    let entries = s2_kit::testing::read_s2log(&log);
    assert_eq!(entries.len(), c.transcript().len());
    for (read, original) in entries.iter().zip(c.transcript()) {
        assert_eq!(read.at, original.at);
        assert_eq!(read.direction, original.direction);
        assert_eq!(read.kind, original.kind);
        assert_eq!(read.to_jsonl(), original.to_jsonl());
    }

    // And the conversation two real engines produced is one an observer finds clean —
    // which is the assertion that makes the analyzer worth having.
    let mut analyzer = Analyzer::new();
    for entry in &entries {
        let observed = analyzer.observe(entry.direction.sender(), &entry.text, entry.at);
        assert!(
            observed.report.is_empty(),
            "{} {}: {}",
            entry.direction.as_str(),
            entry.kind,
            observed.report
        );
    }
    assert_eq!(
        analyzer.active_control_type(),
        Some(ControlType::FillRateBasedControl)
    );
    assert!(analyzer.registry().details.is_some());
}

/// The property that makes a replay worth trusting: a conversation this crate's own two
/// engines produced replays with no findings at all.
///
/// If it ever stops holding, either the engines or the observer is wrong and the test does
/// not say which — they are independent readings of the same rules and have to agree.
#[test]
fn a_conversation_the_engines_produced_replays_clean() {
    use s2_kit::testing::replay;

    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(s2_kit::testing::battery_system(c.now), c.now)
        .unwrap();
    c.pump();
    c.cem
        .instruct(s2_kit::testing::charge("i1", 0.5, c.now), c.now)
        .unwrap();
    c.pump();

    let entries = s2_kit::testing::read_s2log(&c.to_s2log());
    let report = replay(&entries);
    assert!(report.is_conforming(), "{report}");
    assert!(report.findings.is_empty(), "{report}");
    // Every message that needs an answer got one, and `ReceptionStatus` never does.
    let answerable = entries
        .iter()
        .filter(|e| e.kind != s2_kit::MessageKind::ReceptionStatus)
        .count();
    assert_eq!(report.answered, answerable);
}

/// The three things a replay exists to find.
#[test]
fn a_replay_finds_what_a_message_at_a_time_validator_cannot() {
    use s2_kit::testing::{Finding, replay};

    let details = r#"{"message_type":"ResourceManagerDetails","message_id":"d1","resource_id":"battery-1","roles":[{"role":"ENERGY_STORAGE","commodity":"ELECTRICITY"}],"instruction_processing_delay":500,"available_control_types":["FILL_RATE_BASED_CONTROL"],"provides_forecast":false,"provides_power_measurement_types":["ELECTRIC.POWER.3_PHASE_SYMMETRIC"]}"#;
    let line =
        |dir: &str, at: &str, msg: &str| format!(r#"{{"t":"{at}","dir":"{dir}","msg":{msg}}}"#);

    // 1. A message nobody answered.
    let log = line("rm>cem", "2024-01-01T12:00:00Z", details);
    let report = replay(&s2_kit::testing::read_s2log(&log));
    assert!(
        matches!(
            report.findings.as_slice(),
            [Finding::Unanswered { line: 1, .. }]
        ),
        "{report}"
    );
    assert!(!report.is_conforming());

    // 2. A peer that answered differently than this crate would. The instruction names a
    //    control type nobody selected, so `s2-kit` refuses it — and the peer said OK.
    let instruction = r#"{"message_type":"FRBC.Instruction","message_id":"m1","id":"i1","actuator_id":"a1","operation_mode":"om1","operation_mode_factor":0.5,"execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#;
    let ok = r#"{"message_type":"ReceptionStatus","subject_message_id":"m1","status":"OK"}"#;
    let log = [
        line("cem>rm", "2024-01-01T12:00:00Z", instruction),
        line("rm>cem", "2024-01-01T12:00:01Z", ok),
    ]
    .join(
        "
",
    );
    let report = replay(&s2_kit::testing::read_s2log(&log));
    let diff = report
        .findings
        .iter()
        .find(|f| matches!(f, Finding::StatusDiffers { .. }))
        .unwrap_or_else(|| panic!("{report}"));
    match diff {
        Finding::StatusDiffers {
            recorded,
            expected,
            because,
            ..
        } => {
            assert_eq!(*recorded, ReceptionStatusValues::Ok);
            assert_eq!(*expected, ReceptionStatusValues::InvalidContent);
            assert!(
                because
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("S2-STATE-001")
            );
        }
        other => panic!("{other}"),
    }

    // 3. An answer to something nobody sent.
    let stray = r#"{"message_type":"ReceptionStatus","subject_message_id":"ghost","status":"OK"}"#;
    let log = line("rm>cem", "2024-01-01T12:00:00Z", stray);
    let report = replay(&s2_kit::testing::read_s2log(&log));
    assert!(
        matches!(
            report.findings.as_slice(),
            [Finding::UnmatchedAnswer { .. }]
        ),
        "{report}"
    );
}

/// Every counter, asserted against the event that caused it, so none can drift from the
/// thing it counts.
#[test]
fn the_counters_agree_with_what_happened() {
    let mut c = Conversation::battery();
    c.open();
    // The handshake alone: each side sent one message and answered the other's.
    let rm = c.rm.stats();
    assert!(rm.sent >= 2, "a handshake and at least one answer: {rm:?}");
    assert!(rm.received >= 1);
    assert_eq!(rm.refused, 0);
    assert_eq!(rm.undecodable, 0);
    assert_eq!(rm.nacked, 0);
    assert!(rm.acked >= 1, "the manager acknowledged the handshake");
    // Answered inside the conversation's own clock, which never moves here.
    assert_eq!(rm.ack_latency_max_ms, 0);
    assert_eq!(rm.mean_ack_latency_ms(), Some(0));
    assert_eq!(rm.answered(), rm.acked + rm.nacked);

    // A frame that is not S2 at all is counted as received and undecodable, and refused.
    let before = c.rm.stats();
    c.rm.handle_text("{ not json", c.now);
    let after = c.rm.stats();
    assert_eq!(after.received, before.received + 1);
    assert_eq!(after.undecodable, before.undecodable + 1);
    assert_eq!(after.refused, before.refused + 1);

    // A real round trip with time passing is timed.
    let mut c = Conversation::battery();
    c.rm.open(c.now);
    let handshake = c.rm.poll_transmit().expect("a handshake").text;
    c.advance_silently(Duration::from_millis(1500));
    c.cem.handle_text(&handshake, c.now);
    while let Some(out) = c.cem.poll_transmit() {
        c.rm.handle_text(&out.text, c.now);
    }
    let rm = c.rm.stats();
    assert_eq!(rm.acked, 1);
    assert_eq!(rm.ack_latency_max_ms, 1500);
    assert_eq!(rm.mean_ack_latency_ms(), Some(1500));

    // A message refused by the *peer* is a nack, not an ack. Getting one takes a manager
    // that does not check its own outbound messages — which is the whole reason
    // `validate_outbound` defaults to on: the call site is a better place to learn.
    let mut c = Conversation::new(
        RmConfig::default(),
        s2_kit::testing::battery_details(),
        CemConfig::default().without_outbound_validation(),
    );
    c.open();
    let before = c.cem.stats().nacked;
    c.cem
        .send(
            SelectControlType {
                message_id: Id::parse("sel1").unwrap(),
                // The battery offers FRBC only, so the resource refuses this.
                control_type: ControlType::PowerProfileBasedControl,
            },
            c.now,
        )
        .expect("an unvalidating manager sends it");
    c.pump();
    assert_eq!(c.cem.stats().nacked, before + 1);
    assert!(c.rm.stats().refused >= 1);
}

#[test]
fn the_transcript_replays_as_json_lines() {
    let mut c = Conversation::battery();
    c.open();
    let log = c.to_s2log();
    assert!(log.lines().count() >= 4);
    for line in log.lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("a transcript line");
        assert!(value.get("t").is_some());
        assert!(value.get("dir").is_some());
        // And every recorded message decodes on its own.
        let text = value.get("msg").unwrap().to_string();
        decode(&text).expect("a recorded message");
    }
}

#[test]
fn nothing_is_ever_sent_without_being_acknowledged() {
    // The invariant the whole ack ledger exists for, asserted over a whole conversation.
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.cem.instruct(charge("i1", 1.0, c.now), c.now).unwrap();
    c.pump();

    let mut expecting = 0usize;
    let mut acks = 0usize;
    for entry in c.transcript() {
        if entry.kind == MessageKind::ReceptionStatus {
            acks += 1;
        } else {
            expecting += 1;
        }
    }
    assert_eq!(
        expecting, acks,
        "every message except a ReceptionStatus earns exactly one"
    );
}

#[test]
fn a_closed_session_answers_nothing_at_all() {
    // A closed session has no transport, so anything it queues is something nobody will
    // read — and a peer that keeps sending after a close would otherwise grow the outbox
    // of a session that can never drain it. `handle_message` has always refused; the
    // frames that fail to *decode* used to be answered anyway.
    let mut c = Conversation::battery();
    c.open();
    c.pump();
    c.rm.close();
    while c.rm.poll_transmit().is_some() {}

    let before = c.rm.poll_timeout();
    assert_eq!(before, None, "a closed session holds no deadline");

    // Something well-formed, something that is not JSON at all, and something oversized.
    for text in [
        r#"{"message_type":"SelectControlType","message_id":"m9","control_type":"NO_SELECTION"}"#,
        "{ not json",
        "",
    ] {
        let inbound = c.rm.handle_text(text, c.now);
        assert!(inbound.report.is_empty(), "{text}: {:?}", inbound.report);
    }

    assert!(
        c.rm.poll_transmit().is_none(),
        "a closed session must not queue a reception status"
    );
    assert_eq!(c.rm.poll_timeout(), None);
}

#[test]
fn nothing_crosses_before_the_two_sides_have_agreed_a_version() {
    // `S2C §State of communication` starts at `WebSocketConnected` because under S2
    // Connect the version is settled before the socket opens. A bare-WebSocket session
    // has a row before that one — `S2J messages/Handshake` — and a message sent in it is
    // a message whose *schema version* nobody has agreed on yet.
    //
    // The state table used to be keyed by the active control type alone, so `Idle`,
    // `Handshaking` and `Connected` were one row: an RM could put a `PowerMeasurement` on
    // the wire before it had opened the session at all, and the check named after the
    // state never looked at one.
    let mut rm =
        s2_kit::session::RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
    let now = at("2024-01-01T12:00:00Z");
    let measurement = PowerMeasurement {
        message_id: Id::parse("pm1").unwrap(),
        measurement_timestamp: now,
        values: vec![PowerValue::new(
            CommodityQuantity::ElectricPower3PhaseSymmetric,
            10.0,
        )],
    };

    // Before `open`, and while the handshake is in flight.
    assert!(matches!(
        rm.send(measurement.clone(), now),
        Err(s2_kit::session::SendError::NotAllowed { .. })
    ));
    rm.open(now);
    assert!(matches!(rm.state(), SessionState::Handshaking));
    assert!(matches!(
        rm.send(measurement.clone(), now),
        Err(s2_kit::session::SendError::NotAllowed { .. })
    ));

    // And a peer that sends one early is told which rule it broke, rather than having it
    // quietly folded into the session's picture of the resource.
    let early = r#"{"message_type":"SelectControlType","message_id":"m9",
        "control_type":"FILL_RATE_BASED_CONTROL"}"#;
    let inbound = rm.handle_text(early, now);
    assert!(inbound.report.contains(rules::NOT_ALLOWED_IN_STATE));
    assert!(matches!(rm.state(), SessionState::Handshaking));

    // Once the version is agreed, the same message is fine.
    rm.handle_text(
        r#"{"message_type":"HandshakeResponse","message_id":"h1",
            "selected_protocol_version":"1.0.0"}"#,
        now,
    );
    assert!(matches!(rm.state(), SessionState::Connected));
    assert!(rm.send(measurement, now).is_ok());
}

#[test]
fn a_session_under_s2_connect_is_connected_the_moment_it_opens() {
    // The counterpart: S2 Connect settles the version during session initiation, so there
    // is no negotiating row at all and the resource may describe itself immediately.
    let config = RmConfig::default().pre_negotiated(WireProfile::V1_0_0);
    let mut rm = s2_kit::session::RmSession::new(config, s2_kit::testing::battery_details());
    let now = at("2024-01-01T12:00:00Z");
    rm.open(now);
    assert!(matches!(rm.state(), SessionState::Connected));
    assert!(
        rm.send(
            PowerMeasurement {
                message_id: Id::parse("pm1").unwrap(),
                measurement_timestamp: now,
                values: vec![PowerValue::new(
                    CommodityQuantity::ElectricPower3PhaseSymmetric,
                    10.0,
                )],
            },
            now,
        )
        .is_ok()
    );
}

#[test]
fn a_side_hears_its_own_validators_warnings_about_what_it_sent() {
    // `validate_outbound` refuses an outbound message with an *error* at the call site,
    // which is the point of it — and it learned everything the warning rules had to say
    // and threw it away. A manager instructing a transition its own picture of the timers
    // says is blocked (`S2-INST-005`, a warning because that picture is always slightly
    // stale) had no way to find out except by being told by a peer.
    let mut c = Conversation::battery();
    c.open();
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.pump();

    // Discharge, then report the cooldown timer as running: `t4` (discharge → idle) is
    // blocked by it.
    c.rm.send(
        s2_kit::types::frbc::ActuatorStatus {
            message_id: Id::parse("as1").unwrap(),
            actuator_id: Id::parse("actuator1").unwrap(),
            active_operation_mode_id: Id::parse("discharge").unwrap(),
            operation_mode_factor: 0.5,
            previous_operation_mode_id: None,
            transition_timestamp: None,
        },
        c.now,
    )
    .unwrap();
    c.rm.send(
        s2_kit::types::frbc::TimerStatus {
            message_id: Id::parse("ts1").unwrap(),
            timer_id: Id::parse("cooldown").unwrap(),
            actuator_id: Id::parse("actuator1").unwrap(),
            finished_at: c.now.checked_add(Duration::from_secs(600)).unwrap(),
        },
        c.now,
    )
    .unwrap();
    c.pump();
    let _ = c.take_cem_events();

    // Now the manager instructs the blocked transition. It goes out — the rule is a
    // warning, and the RM may well have moved on — but the manager is told.
    c.cem
        .instruct(
            s2_kit::types::frbc::Instruction {
                message_id: Id::parse("mi9").unwrap(),
                id: Id::parse("instr9").unwrap(),
                actuator_id: Id::parse("actuator1").unwrap(),
                operation_mode: Id::parse("idle").unwrap(),
                operation_mode_factor: 0.0,
                execution_time: c.now,
                abnormal_condition: false,
            },
            c.now,
        )
        .expect("a warning does not refuse the send");

    let warned = c.take_cem_events().into_iter().any(|e| {
        matches!(e, CemEvent::OutboundWarnings { report, .. }
            if report.contains(rules::BLOCKED_BY_TIMER))
    });
    assert!(warned, "the sender should hear its own validator");
    assert_eq!(c.cem.stats().sent_with_warnings, 1);
}
