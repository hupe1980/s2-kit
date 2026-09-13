//! Fill Rate Based Control — anything that stores or buffers energy.
//!
//! A battery, an EV, a hot-water tank, a fridge, a building's thermal mass. The Resource
//! Manager describes a **storage** with a fill level, and one or more **actuators** that
//! change it; each actuator is a state machine whose states carry both a power and the
//! rate at which they fill or empty the store.
//!
//! The fill level's unit is the Resource Manager's choice — the standard requires only
//! that the fill *rate* be in that unit per second. The official examples use percent
//! state of charge for an EV and degrees Celsius for a hot-water buffer; `hems` uses
//! kilowatt-hours for every store. This crate imposes nothing and interpolates nothing
//! it was not given.

use alloc::string::String;
use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::common::{PowerRange, Timer, Transition};
use super::{Duration, Id, NumberRange, Timestamp};

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// The storage whose fill level the CEM is planning against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct StorageDescription {
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// What the fill level is measured in — "percentage state of charge", "temperature
    /// in Celsius". For humans; nothing computes with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_level_label: Option<String>,
    /// Whether a [`LeakageBehaviour`] may follow.
    pub provides_leakage_behaviour: bool,
    /// Whether a [`FillLevelTargetProfile`] may follow.
    pub provides_fill_level_target_profile: bool,
    /// Whether a [`UsageForecast`] may follow.
    pub provides_usage_forecast: bool,
    /// The range the fill level should stay within.
    ///
    /// "When the fill_level is not within this range, the Resource Manager can ignore
    /// instructions from the CEM (except during abnormal conditions)."
    pub fill_level_range: NumberRange,
}

/// How an operation mode behaves over one band of the fill level.
///
/// Splitting a mode into elements is how a Resource Manager describes behaviour that is
/// not linear in the fill level — a heat pump whose efficiency falls as the buffer gets
/// hotter, say. The elements' `fill_level_range`s must be contiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct OperationModeElement {
    /// The band of fill level this element describes. Start strictly below end.
    pub fill_level_range: NumberRange,
    /// Change in fill level per second: the start at factor 0, the end at factor 1.
    /// Positive fills the store.
    pub fill_rate: NumberRange,
    /// The power exchanged with the grid, at most one range per commodity quantity.
    pub power_ranges: Vec<PowerRange>,
    /// Cost per second of running in this mode, excluding the commodity itself. The
    /// range expresses uncertainty and is **not** interpolated by the factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_costs: Option<NumberRange>,
}

/// One state of an actuator's state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct OperationMode {
    /// Unique within the actuator description that contains it.
    pub id: Id,
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// How the mode behaves across the fill level. Ranges must be contiguous.
    pub elements: Vec<OperationModeElement>,
    /// Whether this mode may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

impl OperationMode {
    /// The element that applies at `fill_level`, if any band covers it.
    #[must_use]
    pub fn element_at(&self, fill_level: f64) -> Option<&OperationModeElement> {
        self.elements
            .iter()
            .find(|e| e.fill_level_range.contains(fill_level))
    }
}

/// Something the CEM can control that changes the storage's fill level.
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
    pub supported_commodities: Vec<super::common::Commodity>,
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

/// A fill level the CEM should reach within a period.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillLevelTargetProfileElement {
    /// How long this element lasts.
    pub duration: Duration,
    /// The band the fill level must be in while it is active. "The CEM must take
    /// best-effort actions to proactively achieve this target."
    pub fill_level_range: NumberRange,
}

/// How fast the store loses charge on its own, over one band of the fill level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeakageBehaviourElement {
    /// The band this element describes.
    pub fill_level_range: NumberRange,
    /// Fill level lost per second. **Positive means the level decreases.**
    pub leakage_rate: f64,
}

/// Expected demand on the store, over one slice of time.
///
/// As with leakage, a **positive rate means the fill level decreases**.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct UsageForecastElement {
    /// How long this slice lasts.
    pub duration: Duration,
    /// The 100 % upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_rate_upper_limit: Option<f64>,
    /// The 95 % upper bound.
    #[serde(
        rename = "usage_rate_upper_95PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_rate_upper_95ppr: Option<f64>,
    /// The 68 % upper bound.
    #[serde(
        rename = "usage_rate_upper_68PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_rate_upper_68ppr: Option<f64>,
    /// The most likely usage rate.
    pub usage_rate_expected: f64,
    /// The 68 % lower bound.
    #[serde(
        rename = "usage_rate_lower_68PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_rate_lower_68ppr: Option<f64>,
    /// The 95 % lower bound.
    #[serde(
        rename = "usage_rate_lower_95PPR",
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_rate_lower_95ppr: Option<f64>,
    /// The 100 % lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_rate_lower_limit: Option<f64>,
}

impl UsageForecastElement {
    /// The bands from lowest to highest, skipping those that are absent.
    #[must_use]
    pub fn bands_ascending(&self) -> [(&'static str, Option<f64>); 7] {
        [
            ("usage_rate_lower_limit", self.usage_rate_lower_limit),
            ("usage_rate_lower_95PPR", self.usage_rate_lower_95ppr),
            ("usage_rate_lower_68PPR", self.usage_rate_lower_68ppr),
            ("usage_rate_expected", Some(self.usage_rate_expected)),
            ("usage_rate_upper_68PPR", self.usage_rate_upper_68ppr),
            ("usage_rate_upper_95PPR", self.usage_rate_upper_95ppr),
            ("usage_rate_upper_limit", self.usage_rate_upper_limit),
        ]
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// The abstract device: its storage and every actuator that can move it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct SystemDescription {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When this description becomes valid. In the past means immediately.
    pub valid_from: Timestamp,
    /// Every actuator. At least one, at most ten.
    pub actuators: Vec<ActuatorDescription>,
    /// The storage they all affect.
    pub storage: StorageDescription,
}

impl SystemDescription {
    /// The actuator with this id, if the description has one.
    #[must_use]
    pub fn actuator(&self, id: &Id) -> Option<&ActuatorDescription> {
        self.actuators.iter().find(|a| a.id == *id)
    }
}

/// How full the store is now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct StorageStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The present fill level, in the unit the description named.
    pub present_fill_level: f64,
}

impl StorageStatus {
    /// A storage status.
    #[cfg(feature = "uuid")]
    #[must_use]
    pub fn new(present_fill_level: f64) -> Self {
        Self {
            message_id: Id::generate(),
            present_fill_level,
        }
    }
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
    /// Where in that mode's ranges the actuator is running, in `[0, 1]`.
    pub operation_mode_factor: f64,
    /// The mode that was active before. Required unless this is the first mode the
    /// Resource Manager is aware of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_operation_mode_id: Option<Id>,
    /// When the move from the previous mode began. Required under the same condition.
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
    /// When it finishes. In the past means it has finished; "if the timer was never
    /// started, the value can be an arbitrary DateTimeStamp in the past".
    pub finished_at: Timestamp,
}

/// The store's standing losses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct LeakageBehaviour {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When this becomes valid.
    pub valid_from: Timestamp,
    /// Contiguous bands of fill level and the rate lost in each.
    pub elements: Vec<LeakageBehaviourElement>,
}

/// What the user is expected to take out of the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct UsageForecast {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When the forecast starts.
    pub start_time: Timestamp,
    /// The slices, in chronological order.
    pub elements: Vec<UsageForecastElement>,
}

/// Fill levels the CEM should aim for — an EV's departure charge, for instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct FillLevelTargetProfile {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When the profile starts.
    pub start_time: Timestamp,
    /// The targets, in chronological order.
    pub elements: Vec<FillLevelTargetProfileElement>,
}

/// The CEM asking an actuator to change mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier, which the [`InstructionStatusUpdate`] names.
    ///
    /// [`InstructionStatusUpdate`]: super::common::InstructionStatusUpdate
    pub id: Id,
    /// Which actuator.
    pub actuator_id: Id,
    /// Which mode it should move to.
    ///
    /// Note that this field is `operation_mode`, not `operation_mode_id` — FRBC is the
    /// one control type that spells it without the suffix.
    pub operation_mode: Id,
    /// Where in the mode's ranges to run, in `[0, 1]`.
    pub operation_mode_factor: f64,
    /// When to start. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::CommodityQuantity;

    #[test]
    fn the_ev_example_from_the_docs_round_trips() {
        // docs `learn/examples/ev`, the charging operation mode verbatim.
        let json = r#"{
          "id": "om2",
          "diagnostic_label": "Charging",
          "elements": [
            {
              "fill_level_range": { "start_of_range": 0, "end_of_range": 100 },
              "fill_rate": { "start_of_range": 0.00065, "end_of_range": 0.0051 },
              "power_ranges": [
                {
                  "start_of_range": 1400,
                  "end_of_range": 11000,
                  "commodity_quantity": "ELECTRIC.POWER.3_PHASE_SYMMETRIC"
                }
              ]
            }
          ],
          "abnormal_condition_only": false
        }"#;
        let mode: OperationMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode.id, "om2");
        let element = mode.element_at(50.0).unwrap();
        assert_eq!(element.power_ranges.len(), 1);
        // The worked number from the documentation: at 1.4 kW the battery fills at
        // 0.00065 % per second.
        assert!((element.fill_rate.at_factor(0.0) - 0.000_65).abs() < 1e-12);
        assert!((element.power_ranges.first().unwrap().at_factor(0.0) - 1400.0).abs() < 1e-9);
        // No element covers a fill level outside the described band.
        assert!(mode.element_at(101.0).is_none());
    }

    #[test]
    fn a_small_fill_rate_survives_the_json_round_trip() {
        // The value `hems` found: without serde_json's `float_roundtrip` feature this
        // comes back as ...4445, and a learned correction drifts every time it is saved.
        let original = 0.001_319_444_444_444_444_3_f64;
        let element = LeakageBehaviourElement {
            fill_level_range: NumberRange::new(0.0, 100.0),
            leakage_rate: original,
        };
        let json = serde_json::to_string(&element).unwrap();
        let back: LeakageBehaviourElement = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.leakage_rate.to_bits(),
            original.to_bits(),
            "fill rates must round-trip bit for bit"
        );
    }

    #[test]
    fn instruction_keeps_the_operation_mode_field_name() {
        let instruction = Instruction {
            message_id: Id::new_const("m1"),
            id: Id::new_const("instr0"),
            actuator_id: Id::new_const("actuator1"),
            operation_mode: Id::new_const("om1"),
            operation_mode_factor: 1.0,
            execution_time: Timestamp::UNIX_EPOCH,
            abnormal_condition: false,
        };
        let json = serde_json::to_string(&instruction).unwrap();
        assert!(json.contains(r#""operation_mode":"om1""#));
        assert!(!json.contains("operation_mode_id"));
    }

    #[test]
    fn usage_and_leakage_rates_are_positive_when_the_level_falls() {
        // A documented sign convention that is easy to get backwards, so it is asserted
        // here as the place the meaning is recorded.
        let e = UsageForecastElement::builder()
            .duration(Duration::from_secs(900))
            .usage_rate_expected(0.5)
            .build();
        assert!(
            e.usage_rate_expected > 0.0,
            "positive means the store drains"
        );
        let bands: Vec<_> = e.bands_ascending().iter().filter_map(|(_, v)| *v).collect();
        assert_eq!(bands, alloc::vec![0.5]);
    }

    #[test]
    fn power_range_interpolates_between_its_ends() {
        let r = PowerRange::new(500.0, 2000.0, CommodityQuantity::ElectricPowerL1);
        assert!((r.at_factor(1.0) - 2000.0).abs() < 1e-9);
        assert_eq!(r.range(), NumberRange::new(500.0, 2000.0));
    }
}
