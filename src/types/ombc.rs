//! Operation Mode Based Control — a device that can adjust its power, with no
//! constraint on how long the adjustment lasts.
//!
//! A generator, a variable resistor, an SG Ready heat pump. This is the plainest of the
//! three state-machine control types: modes carry a power and nothing else, and there is
//! one machine rather than one per actuator.

use alloc::string::String;
use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::common::{PowerRange, Timer, Transition};
use super::{Id, NumberRange, Timestamp};

/// One state of the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct OperationMode {
    /// Unique within the Resource Manager for the session.
    pub id: Id,
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// The power in this mode: start at factor 0, end at factor 1. At most one range per
    /// commodity quantity.
    pub power_ranges: Vec<PowerRange>,
    /// Cost per second of running in this mode, excluding the commodity. The range
    /// expresses uncertainty, not the factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_costs: Option<NumberRange>,
    /// Whether this mode may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

/// The state machine: every mode, every move between them, every timer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct SystemDescription {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When this description becomes valid.
    pub valid_from: Timestamp,
    /// The states. At least one, at most a hundred.
    pub operation_modes: Vec<OperationMode>,
    /// The permitted moves.
    pub transitions: Vec<Transition>,
    /// The timers those moves start or wait for.
    pub timers: Vec<Timer>,
}

impl SystemDescription {
    /// The operation mode with this id, if the description has one.
    #[must_use]
    pub fn operation_mode(&self, id: &Id) -> Option<&OperationMode> {
        self.operation_modes.iter().find(|m| m.id == *id)
    }

    /// The timer with this id, if the description has one.
    #[must_use]
    pub fn timer(&self, id: &Id) -> Option<&Timer> {
        self.timers.iter().find(|t| t.id == *id)
    }

    /// The transition from `from` to `to`, if one is described.
    #[must_use]
    pub fn transition(&self, from: &Id, to: &Id) -> Option<&Transition> {
        self.transitions
            .iter()
            .find(|t| t.from == *from && t.to == *to)
    }
}

/// Which mode the device is in, and how far into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The mode that is active now.
    pub active_operation_mode_id: Id,
    /// Where in that mode's ranges it is running, in `[0, 1]`.
    pub operation_mode_factor: f64,
    /// The mode that was active before. Required unless this is the first the Resource
    /// Manager is aware of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_operation_mode_id: Option<Id>,
    /// When the move began. Required under the same condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_timestamp: Option<Timestamp>,
}

/// When one of the device's timers finishes.
///
/// Unlike the FRBC and DDBC forms, this one names no actuator — OMBC has a single state
/// machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct TimerStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The timer this is about.
    pub timer_id: Id,
    /// When it finishes.
    pub finished_at: Timestamp,
}

/// The CEM asking the device to change mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier.
    pub id: Id,
    /// When to start. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// The mode to move to.
    pub operation_mode_id: Id,
    /// Where in the mode's ranges to run, in `[0, 1]`.
    pub operation_mode_factor: f64,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::CommodityQuantity;

    #[test]
    fn the_generator_example_from_the_whitepaper() {
        // The whitepaper's diesel generator: off at 0 W, reduced at -1800 W, full at
        // -3000 W. Production is negative.
        let modes = alloc::vec![
            OperationMode {
                id: Id::new_const("off"),
                diagnostic_label: Some("Off".into()),
                power_ranges: alloc::vec![PowerRange::exactly(
                    0.0,
                    CommodityQuantity::ElectricPowerL1
                )],
                running_costs: None,
                abnormal_condition_only: false,
            },
            OperationMode {
                id: Id::new_const("reduced"),
                diagnostic_label: Some("Reduced power".into()),
                power_ranges: alloc::vec![PowerRange::exactly(
                    -1800.0,
                    CommodityQuantity::ElectricPowerL1
                )],
                running_costs: None,
                abnormal_condition_only: false,
            },
        ];
        let system = SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: Timestamp::UNIX_EPOCH,
            operation_modes: modes,
            transitions: alloc::vec![Transition::simple(
                Id::new_const("t1"),
                Id::new_const("off"),
                Id::new_const("reduced")
            )],
            timers: Vec::new(),
        };
        assert!(system.operation_mode(&Id::new_const("reduced")).is_some());
        assert!(
            system
                .transition(&Id::new_const("off"), &Id::new_const("reduced"))
                .is_some()
        );
        // There is no transition the other way, which the state machine is entitled to
        // say and a CEM is expected to respect.
        assert!(
            system
                .transition(&Id::new_const("reduced"), &Id::new_const("off"))
                .is_none()
        );
    }

    #[test]
    fn the_timer_status_has_no_actuator() {
        let json = serde_json::to_string(&TimerStatus {
            message_id: Id::new_const("m1"),
            timer_id: Id::new_const("timer0"),
            finished_at: Timestamp::UNIX_EPOCH,
        })
        .unwrap();
        assert!(!json.contains("actuator_id"));
    }
}
