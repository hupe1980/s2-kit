//! A Resource Manager for a home battery, using Fill Rate Based Control.
//!
//! This is the loop every FRBC Resource Manager writes: describe the abstract device
//! once, report the storage and actuator state as it changes, and act on the
//! instructions that come back. It runs against an in-memory Customer Energy Manager so
//! that `cargo run --example battery_rm` works with no network, no broker and no peer —
//! swap [`Conversation`] for [`s2_kit::io::Driver`] and a WebSocket and the Resource
//! Manager code below is unchanged.
//!
//! ```text
//! cargo run --features testing --example battery_rm
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use s2_kit::model;
use s2_kit::prelude::*;
use s2_kit::testing::{Conversation, battery_details, battery_system};

/// A very simple battery: a state of charge and whatever the manager last asked for.
struct Battery {
    /// Percentage state of charge, which is the fill level this Resource Manager uses.
    state_of_charge: f64,
    /// The operation mode the inverter is in.
    mode: Id,
    /// Where in that mode's range it is running.
    factor: f64,
}

impl Battery {
    fn power(&self) -> f64 {
        // The description says charging runs 0..5000 W and discharging 0..-5000 W.
        match self.mode.as_str() {
            "charge" => 5000.0 * self.factor,
            "discharge" => -5000.0 * self.factor,
            _ => 0.0,
        }
    }
}

fn main() {
    let mut c = Conversation::new(RmConfig::default(), battery_details(), CemConfig::default());
    let mut battery = Battery {
        state_of_charge: 52.0,
        mode: Id::parse("idle").unwrap(),
        factor: 0.0,
    };

    // 1. Handshake and describe the resource. The engine does both.
    c.open();
    println!("state: {:?}", c.rm.state());

    // 2. The manager picks a control type. A real CEM does this when it is ready.
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .expect("the battery offers FRBC");
    c.pump();

    // 3. `Ready` is the signal to publish the abstract device.
    for event in c.take_rm_events() {
        if let RmEvent::Ready { control_type } = event {
            println!("the manager activated {control_type:?}; sending the description");
            c.rm.send(battery_system(c.now), c.now)
                .expect("a description is allowed once a control type is active");
        }
    }
    report(&mut c, &battery);
    c.pump();

    // 4. The manager decides to charge at 60 %.
    let instruction = frbc::Instruction {
        message_id: Id::parse("mi1").unwrap(),
        id: Id::parse("instr0").unwrap(),
        actuator_id: Id::parse("actuator1").unwrap(),
        operation_mode: Id::parse("charge").unwrap(),
        operation_mode_factor: 0.6,
        execution_time: c.now,
        abnormal_condition: false,
    };
    c.cem.instruct(instruction, c.now).expect("a legal charge");
    c.pump();

    // 5. The Resource Manager acts on it. Note that nothing here looks an identifier
    //    up: the engine hands over the instruction already resolved.
    for event in c.take_rm_events() {
        let RmEvent::Instruction(instructed) = event else {
            continue;
        };
        println!("instruction: {}", instructed.explanation);

        if !instructed.explanation.is_actionable() {
            // A timer is in the way, or the mode is for abnormal conditions only. The
            // resource — not the protocol — decides what to do about that.
            c.rm.instruction_status(instructed.id, InstructionStatus::Rejected, c.now)
                .expect("a status is always allowed");
            continue;
        }

        battery.mode = instructed
            .explanation
            .operation_mode
            .as_ref()
            .map_or(battery.mode, |(id, _)| *id);
        battery.factor = instructed.explanation.factor.unwrap_or(0.0);
        println!(
            "  → {} W into the battery, filling at {}%/s",
            battery.power(),
            instructed.explanation.fill_rate.unwrap_or(0.0)
        );

        c.rm.instruction_status(instructed.id, InstructionStatus::Accepted, c.now)
            .expect("a status is always allowed");
        c.rm.instruction_status(instructed.id, InstructionStatus::Started, c.now)
            .expect("a status is always allowed");
    }
    c.pump();

    // 6. Time passes; the state of charge moves and is reported.
    let step = Duration::from_secs(600);
    let system = battery_system(c.now);
    let mode = system.actuators[0]
        .operation_mode(&battery.mode)
        .expect("a described mode");
    battery.state_of_charge = model::project_fill_level(
        mode,
        model::Factor::clamping(battery.factor),
        battery.state_of_charge,
        step,
        None,
        None,
    );
    c.advance(step);
    report(&mut c, &battery);
    c.pump();

    println!(
        "after ten minutes the battery is at {:.2} %",
        battery.state_of_charge
    );

    // 7. Finish politely.
    c.rm.request_session(SessionRequestType::Terminate, Some("done".into()), c.now)
        .expect("a session request is always allowed");
    c.pump();
    println!("state: {:?}", c.rm.state());

    c.assert_no_refusals();
    println!("\n--- transcript ---\n{}", c.to_s2log());
}

/// Publish what the battery knows about itself.
fn report(c: &mut Conversation, battery: &Battery) {
    let now = c.now;
    c.rm.send(
        frbc::StorageStatus::builder()
            .present_fill_level(battery.state_of_charge)
            .build(),
        now,
    )
    .expect("a storage status is allowed once FRBC is active");

    c.rm.send(
        PowerMeasurement::builder()
            .measurement_timestamp(now)
            .values(vec![PowerValue::new(
                CommodityQuantity::ElectricPower3PhaseSymmetric,
                battery.power(),
            )])
            .build(),
        now,
    )
    .expect("a measurement is always allowed");
}
