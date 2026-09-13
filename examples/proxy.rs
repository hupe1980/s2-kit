//! A recording, validating man in the middle.
//!
//! The S2 Analyzer sits between a Resource Manager and a Customer Energy Manager,
//! forwards what crosses, and reports what is wrong with it. This is the same idea in
//! sixty lines, and it shows the three things a proxy needs that an endpoint does not:
//!
//! * **Lenient decoding.** A proxy must be able to carry a message it would refuse to
//!   send. `Strictness::Lenient` strips properties the schema does not define and names
//!   each one, instead of dropping the whole message.
//! * **Both directions in one head.** [`Analyzer`] keeps one registry fed from *both*
//!   sides, which is the only way to say that an instruction names an actuator nobody
//!   described — neither endpoint's own session holds both halves of that.
//! * **Never stopping.** A frame that is not S2 at all is a finding, not the end of the
//!   log: a proxy that stops at the first bad frame stops exactly where it was needed.
//!
//! ```text
//! cargo run --features testing --example proxy
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::print_stdout
)]

use s2_kit::prelude::*;
use s2_kit::session::Analyzer;
use s2_kit::types::common::EnergyManagementRole::{Cem, Rm};

fn main() {
    let now: Timestamp = "2024-01-01T12:00:00Z".parse().unwrap();

    // Traffic as it might arrive from two implementations that do not agree with each
    // other, or with the standard. The direction matters: half these findings depend on
    // who was speaking.
    let traffic = [
        (
            Rm,
            r#"{"message_type":"ResourceManagerDetails","message_id":"d1",
            "resource_id":"battery-1","roles":[{"role":"ENERGY_STORAGE","commodity":"ELECTRICITY"}],
            "instruction_processing_delay":500,
            "available_control_types":["FILL_RATE_BASED_CONTROL"],
            "provides_forecast":false,
            "provides_power_measurement_types":["ELECTRIC.POWER.3_PHASE_SYMMETRIC"]}"#,
        ),
        (
            Cem,
            r#"{"message_type":"SelectControlType","message_id":"c1",
            "control_type":"FILL_RATE_BASED_CONTROL"}"#,
        ),
        (
            Rm,
            r#"{"message_type":"FRBC.SystemDescription","message_id":"s1",
            "valid_from":"2024-01-01T00:00:00Z",
            "actuators":[{"id":"actuator1","supported_commodities":["ELECTRICITY"],
              "operation_modes":[{"id":"om1","elements":[
                {"fill_level_range":{"start_of_range":0,"end_of_range":100},
                 "fill_rate":{"start_of_range":0,"end_of_range":0.001},
                 "power_ranges":[{"start_of_range":0,"end_of_range":5000,
                   "commodity_quantity":"ELECTRIC.POWER.3_PHASE_SYMMETRIC"}]}],
                "abnormal_condition_only":false}],
              "transitions":[],"timers":[]}],
            "storage":{"provides_leakage_behaviour":false,
              "provides_fill_level_target_profile":false,"provides_usage_forecast":false,
              "fill_level_range":{"start_of_range":0,"end_of_range":100}}}"#,
        ),
        // A vendor extension: legal for the vendor, unknown to the schema.
        (
            Rm,
            r#"{"message_type":"FRBC.StorageStatus","message_id":"m2",
            "present_fill_level":48.0,"acme_cell_temperature":31.4}"#,
        ),
        // Schema-valid and semantically wrong: a factor outside [0, 1].
        (
            Cem,
            r#"{"message_type":"FRBC.Instruction","message_id":"m3","id":"i1",
            "actuator_id":"actuator1","operation_mode":"om1","operation_mode_factor":1.3,
            "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#,
        ),
        // Schema-valid, and wrong only in the light of what crossed three messages ago:
        // no such actuator was ever described. Nothing but a stateful observer sees this.
        (
            Cem,
            r#"{"message_type":"FRBC.Instruction","message_id":"m4","id":"i2",
            "actuator_id":"actuator9","operation_mode":"om1","operation_mode_factor":0.5,
            "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}"#,
        ),
        // The resource answering with something only the manager may send.
        (
            Rm,
            r#"{"message_type":"SelectControlType","message_id":"m5",
            "control_type":"NOT_CONTROLABLE"}"#,
        ),
        // Not S2 at all.
        (Cem, r#"{"message_type":"FRBC.Surprise","message_id":"m6"}"#),
    ];

    let mut analyzer = Analyzer::new();
    let mut forwarded = 0;
    let mut findings = 0;

    for (sender, text) in traffic {
        let observed = analyzer.observe(sender, text, now);
        let who = if sender == Rm { "rm >" } else { "cem>" };
        match &observed.message {
            Some(message) => {
                for violation in observed.report.violations() {
                    println!("  ! {violation}");
                    findings += 1;
                }
                // A proxy forwards even what it would refuse to send: the peer that
                // needs to see the problem is the one at the other end.
                println!("{who} {}", encode(message));
                forwarded += 1;
            }
            None => {
                for violation in observed.report.violations() {
                    println!("  ✗ {violation}");
                    findings += 1;
                }
            }
        }
    }

    println!(
        "\nforwarded {forwarded} of {} messages, {findings} finding(s)",
        traffic.len()
    );
    println!(
        "the conversation described {} actuator(s)",
        analyzer
            .registry()
            .frbc
            .as_ref()
            .map_or(0, |d| d.actuators.len())
    );
}
