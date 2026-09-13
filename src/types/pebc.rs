//! Power Envelope Based Control — a device that cannot be driven, only bounded.
//!
//! Curtailable PV, a wallbox that can only be limited. The Resource Manager publishes
//! [`PowerConstraints`] saying *within what bounds* the CEM may set a limit, and
//! optionally an [`EnergyConstraint`] saying how much energy must still flow over a
//! period. The CEM answers with an [`Instruction`] carrying the actual envelope.
//!
//! Remember the sign convention: production is negative. A 4 kWp array that may be
//! curtailed freely has a `LOWER_LIMIT` range of `-4000..0` and an `UPPER_LIMIT` range of
//! `0..0` (docs `learn/examples/pv`).

use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::common::CommodityQuantity;
use super::{Duration, Id, NumberRange, Timestamp};

/// Which end of an envelope a constraint or limit describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PowerEnvelopeLimitType {
    /// The ceiling: the Resource Manager is asked to stay at or below it.
    UpperLimit,
    /// The floor: the Resource Manager is asked to stay at or above it.
    LowerLimit,
}

/// What happens to the energy a limit prevents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PowerEnvelopeConsequenceType {
    /// "the limited load or generated will be lost and not reappear in the future" — a
    /// curtailed solar panel.
    Vanish,
    /// "the limited load or generation will be postponed to a later moment" — a deferred
    /// charge.
    Defer,
}

/// One bound the CEM is allowed to choose within.
///
/// Several ranges with the same quantity and limit type are explicitly allowed, and
/// together they describe a union — a device that can curtail to 0 or to between 1 and
/// 4 kW but not in between.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct AllowedLimitRange {
    /// Which quantity this applies to.
    pub commodity_quantity: CommodityQuantity,
    /// Whether it bounds the ceiling or the floor.
    pub limit_type: PowerEnvelopeLimitType,
    /// The values the CEM may choose between, start at or below end.
    pub range_boundary: NumberRange,
    /// Whether this range may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

/// One step of an envelope: a ceiling and a floor, for a while.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerEnvelopeElement {
    /// How long this step lasts.
    pub duration: Duration,
    /// The ceiling. Must be at or above `lower_limit`.
    pub upper_limit: f64,
    /// The floor. Must be at or below `upper_limit`.
    pub lower_limit: f64,
}

impl PowerEnvelopeElement {
    /// A step.
    #[must_use]
    pub const fn new(duration: Duration, lower_limit: f64, upper_limit: f64) -> Self {
        Self {
            duration,
            upper_limit,
            lower_limit,
        }
    }
}

/// A sequence of steps bounding one commodity quantity over time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerEnvelope {
    /// Unique within the Resource Manager for the session.
    pub id: Id,
    /// Which quantity this envelope bounds.
    pub commodity_quantity: CommodityQuantity,
    /// The steps, in chronological order.
    pub power_envelope_elements: Vec<PowerEnvelopeElement>,
}

impl PowerEnvelope {
    /// The total time the envelope covers.
    #[must_use]
    pub fn total_duration(&self) -> Duration {
        self.power_envelope_elements
            .iter()
            .fold(Duration::ZERO, |acc, e| {
                acc.checked_add(e.duration).unwrap_or(acc)
            })
    }

    /// The step in force `offset` after the envelope's start.
    ///
    /// `None` once the envelope has run out — at which point the Resource Manager is
    /// unbounded again, which is why this is an `Option` rather than a clamped edge.
    #[must_use]
    pub fn element_at(&self, offset: Duration) -> Option<&PowerEnvelopeElement> {
        let mut elapsed = 0u64;
        let target = offset.as_millis();
        for element in &self.power_envelope_elements {
            let end = elapsed.saturating_add(element.duration.as_millis());
            if target < end {
                return Some(element);
            }
            elapsed = end;
        }
        None
    }
}

/// The bounds within which the CEM may set an envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerConstraints {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The constraints' own identifier, which an [`Instruction`] refers back to.
    pub id: Id,
    /// When they become valid.
    pub valid_from: Timestamp,
    /// When they stop being valid. Absent means open-ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<Timestamp>,
    /// What happens to energy a limit prevents.
    pub consequence_type: PowerEnvelopeConsequenceType,
    /// At least one `UPPER_LIMIT` range and one `LOWER_LIMIT` range.
    pub allowed_limit_ranges: Vec<AllowedLimitRange>,
}

impl PowerConstraints {
    /// Whether these constraints are in force at `now`.
    #[must_use]
    pub fn is_valid_at(&self, now: Timestamp) -> bool {
        now >= self.valid_from && self.valid_until.is_none_or(|until| now < until)
    }

    /// The allowed ranges for one quantity and limit type.
    pub fn ranges_for(
        &self,
        quantity: CommodityQuantity,
        limit_type: PowerEnvelopeLimitType,
    ) -> impl Iterator<Item = &AllowedLimitRange> {
        self.allowed_limit_ranges
            .iter()
            .filter(move |r| r.commodity_quantity == quantity && r.limit_type == limit_type)
    }
}

/// How much energy must still flow over a period, whatever envelope is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct EnergyConstraint {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The constraint's own identifier.
    pub id: Id,
    /// When it becomes valid.
    pub valid_from: Timestamp,
    /// When it stops being valid.
    pub valid_until: Timestamp,
    /// The highest average power over the period. Multiply by the period for energy.
    pub upper_average_power: f64,
    /// The lowest average power over the period.
    ///
    /// The schema's description for this field says it "Must be greater than or equal to
    /// lower_average_power", which is a copy-paste of the line above it; it must of
    /// course be *at most* `upper_average_power` (erratum E3).
    pub lower_average_power: f64,
    /// Which quantity both averages refer to.
    pub commodity_quantity: CommodityQuantity,
}

/// The envelope the CEM wants followed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier.
    pub id: Id,
    /// When the envelope starts. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
    /// The [`PowerConstraints`] this envelope was chosen within.
    pub power_constraints_id: Id,
    /// At most one envelope per commodity quantity.
    pub power_envelopes: Vec<PowerEnvelope>,
}

impl Instruction {
    /// The envelope for one quantity, if this instruction carries one.
    #[must_use]
    pub fn envelope_for(&self, quantity: CommodityQuantity) -> Option<&PowerEnvelope> {
        self.power_envelopes
            .iter()
            .find(|e| e.commodity_quantity == quantity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pv_example_from_the_docs_parses() {
        // docs `learn/examples/pv`, verbatim.
        let json = r#"{
          "message_id": "aaaaaaaa-0000-0000-0000-000000000001",
          "id": "powerConstraint1",
          "valid_from": "2024-08-24T14:15:22Z",
          "valid_until": "2024-08-25T14:15:22Z",
          "consequence_type": "VANISH",
          "allowed_limit_ranges": [
            {
              "commodity_quantity": "ELECTRIC.POWER.L1",
              "limit_type": "LOWER_LIMIT",
              "range_boundary": { "start_of_range": -4000, "end_of_range": 0 },
              "abnormal_condition_only": false
            },
            {
              "commodity_quantity": "ELECTRIC.POWER.L1",
              "limit_type": "UPPER_LIMIT",
              "range_boundary": { "start_of_range": 0, "end_of_range": 0 },
              "abnormal_condition_only": false
            }
          ]
        }"#;
        let c: PowerConstraints = serde_json::from_str(json).unwrap();
        assert_eq!(c.id, "powerConstraint1");
        assert_eq!(c.consequence_type, PowerEnvelopeConsequenceType::Vanish);
        let lower: Vec<_> = c
            .ranges_for(
                CommodityQuantity::ElectricPowerL1,
                PowerEnvelopeLimitType::LowerLimit,
            )
            .collect();
        assert_eq!(lower.len(), 1);
        // Production is negative: the array may be curtailed anywhere in -4000..0.
        assert!(lower[0].range_boundary.contains(-2000.0));
        assert!(!lower[0].range_boundary.contains(-5000.0));
    }

    #[test]
    fn validity_window_is_half_open_and_tolerates_an_open_end() {
        let from: Timestamp = "2024-08-24T14:00:00Z".parse().unwrap();
        let until: Timestamp = "2024-08-24T15:00:00Z".parse().unwrap();
        let c = PowerConstraints {
            message_id: Id::new_const("m1"),
            id: Id::new_const("pc1"),
            valid_from: from,
            valid_until: Some(until),
            consequence_type: PowerEnvelopeConsequenceType::Vanish,
            allowed_limit_ranges: Vec::new(),
        };
        assert!(!c.is_valid_at("2024-08-24T13:59:59Z".parse().unwrap()));
        assert!(c.is_valid_at(from));
        assert!(c.is_valid_at("2024-08-24T14:59:59Z".parse().unwrap()));
        assert!(!c.is_valid_at(until));

        let open = PowerConstraints {
            valid_until: None,
            ..c
        };
        assert!(open.is_valid_at("2099-01-01T00:00:00Z".parse().unwrap()));
    }

    #[test]
    fn an_envelope_runs_out_rather_than_clamping() {
        let e = PowerEnvelope {
            id: Id::new_const("pe1"),
            commodity_quantity: CommodityQuantity::ElectricPowerL1,
            power_envelope_elements: alloc::vec![
                PowerEnvelopeElement::new(Duration::from_secs(60), -2000.0, 0.0),
                PowerEnvelopeElement::new(Duration::from_secs(60), -1000.0, 0.0),
            ],
        };
        assert_eq!(e.total_duration(), Duration::from_secs(120));
        assert_eq!(e.element_at(Duration::ZERO).unwrap().lower_limit, -2000.0);
        assert_eq!(
            e.element_at(Duration::from_secs(59)).unwrap().lower_limit,
            -2000.0
        );
        assert_eq!(
            e.element_at(Duration::from_secs(60)).unwrap().lower_limit,
            -1000.0
        );
        // Past the end the Resource Manager is unbounded again, which is a different
        // thing from "bounded by the last step for ever".
        assert!(e.element_at(Duration::from_secs(120)).is_none());
    }
}
