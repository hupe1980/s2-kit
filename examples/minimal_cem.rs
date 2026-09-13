//! A Customer Energy Manager in about a hundred lines.
//!
//! The manager's side is the mirror of the resource's: it waits to be described, picks
//! a control type, and instructs. Everything protocol-shaped — the handshake, the
//! acknowledgements, the state table, the registry of what the resource published — is
//! the engine's.
//!
//! This one implements the simplest policy that is not trivial: keep the battery's state
//! of charge inside a band.
//!
//! ```text
//! cargo run --features testing --example minimal_cem
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use s2_kit::prelude::*;
use s2_kit::testing::{Conversation, battery_details, battery_system};

/// Keep the store between these two levels.
const FLOOR: f64 = 30.0;
const CEILING: f64 = 80.0;

fn main() {
    let mut c = Conversation::new(RmConfig::default(), battery_details(), CemConfig::default());
    c.open();

    // 1. What did the resource say it is?
    for event in c.take_cem_events() {
        if let CemEvent::ResourceDescribed(details) = event {
            println!(
                "{} offers {:?}",
                details.name.as_deref().unwrap_or("a resource"),
                details.available_control_types
            );
            // Pick the richest control type it offers. FRBC tells a manager the most.
            let choice = [
                ControlType::FillRateBasedControl,
                ControlType::DemandDrivenBasedControl,
                ControlType::PowerProfileBasedControl,
                ControlType::OperationModeBasedControl,
                ControlType::PowerEnvelopeBasedControl,
                ControlType::NotControllable,
            ]
            .into_iter()
            .find(|c| details.available_control_types.contains(c))
            .expect("a resource offers at least one control type");
            c.cem
                .select_control_type(choice, c.now)
                .expect("a selection is always allowed");
        }
    }
    c.pump();

    // The resource publishes its abstract device once the control type is active.
    c.rm.send(battery_system(c.now), c.now).unwrap();
    c.pump();

    // 2. Drive it. Each round: the resource reports, the manager decides.
    for (round, level) in [(1, 20.0), (2, 55.0), (3, 90.0)] {
        // The builders generate a fresh `message_id`, which matters: an engine answers a
        // repeated identifier again without processing it twice, so reusing one would
        // quietly drop every report after the first.
        c.rm.send(
            frbc::StorageStatus::builder()
                .present_fill_level(level)
                .build(),
            c.now,
        )
        .unwrap();
        c.pump();

        let Some(system) = c.cem.registry().frbc.clone() else {
            continue;
        };
        let actuator = &system.actuators[0];
        let fill_level = c.cem.registry().fill_level.unwrap_or(50.0);

        // The policy, in three lines: charge when low, discharge when high, idle between.
        let (mode, factor) = if fill_level < FLOOR {
            ("charge", 1.0)
        } else if fill_level > CEILING {
            ("discharge", 1.0)
        } else {
            ("idle", 0.0)
        };
        let mode = Id::parse(mode).unwrap();
        assert!(actuator.operation_mode(&mode).is_some(), "a described mode");

        let instruction = frbc::Instruction::builder()
            .id(Id::parse(&format!("instr{round}")).unwrap())
            .actuator_id(actuator.id)
            .operation_mode(mode)
            .operation_mode_factor(factor)
            .execution_time(c.now)
            .abnormal_condition(false)
            .build();

        match c.cem.instruct(instruction, c.now) {
            Ok(handle) => println!("round {round}: at {fill_level} % → {mode} ({handle:?})"),
            // The manager learns at the call site rather than from a rejection later.
            Err(e) => println!("round {round}: refused before sending — {e}"),
        }
        c.pump();

        // The resource answers twice: a reception status, then what it did.
        for event in c.take_rm_events() {
            if let RmEvent::Instruction(i) = event {
                c.rm.instruction_status(i.id, InstructionStatus::Accepted, c.now)
                    .unwrap();
            }
        }
        c.pump();
        for event in c.take_cem_events() {
            if let CemEvent::InstructionStatus(u) = event {
                println!("  {} → {:?}", u.instruction_id, u.status_type);
            }
        }
        c.advance(Duration::from_secs(300));
    }

    c.assert_no_refusals();
}
