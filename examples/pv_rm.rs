//! A Resource Manager for a curtailable PV inverter, using Power Envelope Based Control.
//!
//! PEBC is the control type for a device that cannot be driven, only bounded. The
//! inverter publishes the limits a manager may choose between; the manager answers with
//! an envelope; the inverter clamps its output to it.
//!
//! Remember the sign convention — production is negative — which is why a 4 kWp array
//! publishes a `LOWER_LIMIT` range of `-4000..0`.
//!
//! ```text
//! cargo run --features testing --example pv_rm
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
use s2_kit::testing::{Conversation, pv_constraints, pv_details};

fn main() {
    let mut c = Conversation::new(RmConfig::default(), pv_details(), CemConfig::default());
    c.open();

    c.cem
        .select_control_type(ControlType::PowerEnvelopeBasedControl, c.now)
        .expect("the inverter offers PEBC");
    c.pump();

    // What the sun is doing, before any curtailment.
    let uncurtailed = -3450.6_f64;

    for event in c.take_rm_events() {
        if let RmEvent::Ready { .. } = event {
            c.rm.send(pv_constraints(c.now), c.now)
                .expect("constraints are allowed once PEBC is active");
            // The inverter also offers a forecast, which it said it provides.
            c.rm.send(
                PowerForecast {
                    message_id: Id::parse("pf1").unwrap(),
                    start_time: c.now,
                    elements: vec![PowerForecastElement {
                        duration: Duration::from_secs(3600),
                        power_values: vec![PowerForecastValue {
                            value_upper_limit: Some(-3400.0),
                            value_lower_limit: Some(-3500.0),
                            ..PowerForecastValue::expected(
                                uncurtailed,
                                CommodityQuantity::ElectricPowerL1,
                            )
                        }],
                    }],
                },
                c.now,
            )
            .expect("a forecast is always allowed");
        }
    }
    c.pump();

    // The manager curtails to 2 kW for an hour.
    let instruction = pebc::Instruction {
        message_id: Id::parse("mi1").unwrap(),
        id: Id::parse("envelope1").unwrap(),
        execution_time: c.now,
        abnormal_condition: false,
        power_constraints_id: Id::parse("powerConstraint1").unwrap(),
        power_envelopes: vec![pebc::PowerEnvelope {
            id: Id::parse("pe_1").unwrap(),
            commodity_quantity: CommodityQuantity::ElectricPowerL1,
            power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                Duration::from_secs(3600),
                -2000.0,
                0.0,
            )],
        }],
    };
    c.cem
        .instruct(instruction, c.now)
        .expect("inside the published range");
    c.pump();

    let mut envelope = None;
    for event in c.take_rm_events() {
        if let RmEvent::Instruction(instructed) = event {
            println!("instruction: {}", instructed.explanation);
            if let Message::PebcInstruction(i) = &instructed.message {
                envelope = Some((**i).clone());
            }
            c.rm.instruction_status(instructed.id, InstructionStatus::Succeeded, c.now)
                .expect("a status is always allowed");
        }
    }
    c.pump();

    // Apply it: the inverter reports the clamped power, not the sun's.
    let envelope = envelope.expect("an envelope arrived");
    let limits = model::limits_at(&envelope, CommodityQuantity::ElectricPowerL1, c.now)
        .expect("the envelope is in force now");
    let produced = uncurtailed.max(limits.lower).min(limits.upper);
    println!(
        "the sun offers {uncurtailed} W; the envelope allows {}..{}; the inverter produces {produced} W",
        limits.lower, limits.upper
    );
    assert!(model::within(limits, produced));

    c.rm.send(
        PowerMeasurement {
            message_id: Id::parse("pm1").unwrap(),
            measurement_timestamp: c.now,
            values: vec![PowerValue::new(
                CommodityQuantity::ElectricPowerL1,
                produced,
            )],
        },
        c.now,
    )
    .expect("a measurement is always allowed");
    c.pump();

    // Once the envelope runs out the inverter is unbounded again — which is a different
    // thing from being held at its last step for ever.
    let later = c.now.checked_add(Duration::from_secs(3601)).unwrap();
    assert!(model::limits_at(&envelope, CommodityQuantity::ElectricPowerL1, later).is_none());
    println!("an hour later the envelope has run out and the inverter is free again");

    c.assert_no_refusals();
}
