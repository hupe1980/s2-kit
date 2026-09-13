//! Demand Driven Based Control — a device that must meet a demand but is flexible in
//! how it meets it.
//!
//! A hybrid heat pump is the standard's own example: it must deliver a given amount of
//! heat, and can choose between the heat pump and the gas boiler to do it. Like FRBC it
//! has actuators that are state machines; unlike FRBC, a mode's states do not describe
//! an effect on a store but a **supply rate** the CEM can match against the demand.
//!
//! DDBC is also the one place where the two tagged versions of S2 JSON differ. In
//! `v0.0.2-beta` the present demand rate was a required field of
//! [`SystemDescription`]; in `v1.0.0` it is a message of its own,
//! [`PresentDemandStatus`]. See [`WireProfile`](crate::types::WireProfile).

use alloc::string::String;
use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::common::{Commodity, PowerRange, Timer, Transition};
use super::{Duration, Id, NumberRange, Timestamp};

/// One state of an actuator's state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct OperationMode {
    /// Unique within the actuator description that contains it.
    ///
    /// The wire spells this field with a capital `I` — the only field in the whole of
    /// S2 JSON that does (erratum E4).
    #[serde(rename = "Id")]
    pub id: Id,
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// The power in this mode: start at factor 0, end at factor 1.
    pub power_ranges: Vec<PowerRange>,
    /// The supply rate this mode can deliver, which the CEM matches against the demand.
    /// Start at factor 0, end at factor 1.
    pub supply_range: NumberRange,
    /// Cost per second of running in this mode, excluding the commodity. The range
    /// expresses uncertainty, not the factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_costs: Option<NumberRange>,
    /// Whether this mode may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

/// One of the ways the system can meet its demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct ActuatorDescription {
    /// Unique within the Resource Manager for the session.
    pub id: Id,
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// The commodities this actuator's modes use.
    ///
    /// The wire drops an `i` — `supported_commodites` (erratum E5).
    #[serde(rename = "supported_commodites")]
    pub supported_commodities: Vec<Commodity>,
    /// The states of the machine.
    pub operation_modes: Vec<OperationMode>,
    /// The permitted moves between them.
    pub transitions: Vec<Transition>,
    /// Timers those moves start or wait for.
    pub timers: Vec<Timer>,
}

impl ActuatorDescription {
    /// The operation mode with this id, if the actuator has one.
    #[must_use]
    pub fn operation_mode(&self, id: &Id) -> Option<&OperationMode> {
        self.operation_modes.iter().find(|m| m.id == *id)
    }

    /// The timer with this id, if the actuator has one.
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

/// Expected demand over one slice of time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct AverageDemandRateForecastElement {
    /// How long this slice lasts.
    pub duration: Duration,
    /// The 100 % upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demand_rate_upper_limit: Option<f64>,
    /// The 95 % upper bound.
    #[serde(
        rename = "demand_rate_upper_95PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub demand_rate_upper_95ppr: Option<f64>,
    /// The 68 % upper bound.
    #[serde(
        rename = "demand_rate_upper_68PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub demand_rate_upper_68ppr: Option<f64>,
    /// The most likely demand rate.
    pub demand_rate_expected: f64,
    /// The 68 % lower bound.
    #[serde(
        rename = "demand_rate_lower_68PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub demand_rate_lower_68ppr: Option<f64>,
    /// The 95 % lower bound.
    #[serde(
        rename = "demand_rate_lower_95PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub demand_rate_lower_95ppr: Option<f64>,
    /// The 100 % lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demand_rate_lower_limit: Option<f64>,
}

impl AverageDemandRateForecastElement {
    /// The bands from lowest to highest, skipping those that are absent.
    #[must_use]
    pub fn bands_ascending(&self) -> [(&'static str, Option<f64>); 7] {
        [
            ("demand_rate_lower_limit", self.demand_rate_lower_limit),
            ("demand_rate_lower_95PPR", self.demand_rate_lower_95ppr),
            ("demand_rate_lower_68PPR", self.demand_rate_lower_68ppr),
            ("demand_rate_expected", Some(self.demand_rate_expected)),
            ("demand_rate_upper_68PPR", self.demand_rate_upper_68ppr),
            ("demand_rate_upper_95PPR", self.demand_rate_upper_95ppr),
            ("demand_rate_upper_limit", self.demand_rate_upper_limit),
        ]
    }
}

/// Every actuator that can meet the demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct SystemDescription {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When this description becomes valid.
    pub valid_from: Timestamp,
    /// Every actuator. At least one, at most ten.
    pub actuators: Vec<ActuatorDescription>,
    /// The demand that must be satisfied right now.
    ///
    /// **Required in `v0.0.2-beta` and absent in `v1.0.0`**, where it became
    /// [`PresentDemandStatus`]. The codec enforces whichever the negotiated profile
    /// requires, so this is the one field whose optionality is a function of the version
    ///.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub present_demand_rate: Option<NumberRange>,
    /// Whether an [`AverageDemandRateForecast`] may follow.
    pub provides_average_demand_rate_forecast: bool,
}

impl SystemDescription {
    /// The actuator with this id, if the description has one.
    #[must_use]
    pub fn actuator(&self, id: &Id) -> Option<&ActuatorDescription> {
        self.actuators.iter().find(|a| a.id == *id)
    }
}

/// The demand that must be satisfied right now.
///
/// New in S2 JSON v1.0.0; in `v0.0.2-beta` this was a field of [`SystemDescription`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PresentDemandStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The demand rate the system has to satisfy.
    pub present_demand_rate: NumberRange,
}

/// Which operation mode an actuator is in, and how far into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct ActuatorStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The actuator this is about.
    pub actuator_id: Id,
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

/// When one of an actuator's timers finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct TimerStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The timer this is about.
    pub timer_id: Id,
    /// The actuator it belongs to.
    pub actuator_id: Id,
    /// When it finishes.
    pub finished_at: Timestamp,
}

/// What the system expects to be asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct AverageDemandRateForecast {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When the forecast starts.
    pub start_time: Timestamp,
    /// The slices, in chronological order.
    pub elements: Vec<AverageDemandRateForecastElement>,
}

/// The CEM asking an actuator to change mode.
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
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
    /// Which actuator.
    pub actuator_id: Id,
    /// Which mode it should move to.
    pub operation_mode_id: Id,
    /// Where in the mode's ranges to run, in `[0, 1]`.
    pub operation_mode_factor: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::CommodityQuantity;

    #[test]
    fn the_two_misspellings_are_on_the_wire_and_not_in_rust() {
        let mode = OperationMode {
            id: Id::new_const("heatpump"),
            diagnostic_label: None,
            power_ranges: alloc::vec![PowerRange::exactly(
                2000.0,
                CommodityQuantity::ElectricPowerL1
            )],
            supply_range: NumberRange::new(0.0, 6000.0),
            running_costs: None,
            abnormal_condition_only: false,
        };
        let actuator = ActuatorDescription {
            id: Id::new_const("a1"),
            diagnostic_label: None,
            supported_commodities: alloc::vec![Commodity::Electricity],
            operation_modes: alloc::vec![mode],
            transitions: Vec::new(),
            timers: Vec::new(),
        };
        let json = serde_json::to_string(&actuator).unwrap();
        assert!(json.contains("\"supported_commodites\""), "{json}");
        assert!(json.contains("\"Id\":\"heatpump\""), "{json}");
        assert!(!json.contains("\"supported_commodities\""));
        let back: ActuatorDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actuator);
    }

    #[test]
    fn the_demand_rate_is_a_field_in_beta_and_a_message_in_one_zero() {
        // v1.0.0 shape: no such field.
        let one_zero = SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: Timestamp::UNIX_EPOCH,
            actuators: Vec::new(),
            present_demand_rate: None,
            provides_average_demand_rate_forecast: false,
        };
        let json = serde_json::to_string(&one_zero).unwrap();
        assert!(!json.contains("present_demand_rate"));

        // beta shape: the field is there, between `actuators` and the forecast flag,
        // which is where the beta schema declares it.
        let beta = SystemDescription {
            present_demand_rate: Some(NumberRange::new(0.0, 6000.0)),
            ..one_zero
        };
        let json = serde_json::to_string(&beta).unwrap();
        let demand_at = json.find("present_demand_rate").unwrap();
        let actuators_at = json.find("actuators").unwrap();
        let provides_at = json.find("provides_average").unwrap();
        assert!(actuators_at < demand_at && demand_at < provides_at);
    }
}
