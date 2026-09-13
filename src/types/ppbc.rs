//! Power Profile Based Control — a task that must be done, but not necessarily now.
//!
//! A washing machine, a dishwasher, a tumble dryer. The Resource Manager publishes a
//! [`PowerProfileDefinition`]: a window (`start_time` to `end_time`) and one or more
//! **containers**, each offering **alternative sequences** the CEM may choose between —
//! a fast hot wash and a slow one, say. The CEM schedules one sequence per container,
//! and may interrupt and resume a sequence that says it is interruptible.

use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::common::PowerForecastValue;
use super::{Duration, Id, Timestamp};

/// How far a container's chosen sequence has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PowerSequenceStatus {
    /// Nothing in this container is scheduled.
    NotScheduled,
    /// The chosen sequence will run in the future.
    Scheduled,
    /// It is running.
    Executing,
    /// It is running but paused, and will continue.
    Interrupted,
    /// It finished successfully.
    Finished,
    /// The device abandoned it; it will not continue.
    Aborted,
}

impl PowerSequenceStatus {
    /// Whether a sequence has been chosen at all.
    #[must_use]
    pub const fn has_selection(self) -> bool {
        !matches!(self, Self::NotScheduled)
    }

    /// Whether the sequence has started — the condition under which
    /// `PowerSequenceContainerStatus::progress` must be present.
    #[must_use]
    pub const fn has_started(self) -> bool {
        matches!(
            self,
            Self::Executing | Self::Interrupted | Self::Finished | Self::Aborted
        )
    }
}

/// One step of a sequence: a power, for a while.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerSequenceElement {
    /// How long this step lasts.
    pub duration: Duration,
    /// The power drawn or produced, at most one value per commodity quantity.
    pub power_values: Vec<PowerForecastValue>,
}

/// One way of performing the task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerSequence {
    /// Unique within the container that holds it.
    pub id: Id,
    /// The steps, in chronological order.
    pub elements: Vec<PowerSequenceElement>,
    /// Whether the CEM may pause it once started.
    pub is_interruptible: bool,
    /// The longest the device may be paused between the end of the previous sequence and
    /// the start of this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_pause_before: Option<Duration>,
    /// Whether this sequence may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

impl PowerSequence {
    /// How long the whole sequence takes.
    #[must_use]
    pub fn total_duration(&self) -> Duration {
        self.elements.iter().fold(Duration::ZERO, |acc, e| {
            acc.checked_add(e.duration).unwrap_or(acc)
        })
    }
}

/// A set of alternatives, exactly one of which the CEM chooses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerSequenceContainer {
    /// Unique within the profile that holds it.
    pub id: Id,
    /// The alternatives. At least one.
    pub power_sequences: Vec<PowerSequence>,
}

impl PowerSequenceContainer {
    /// The sequence with this id, if the container offers one.
    #[must_use]
    pub fn sequence(&self, id: &Id) -> Option<&PowerSequence> {
        self.power_sequences.iter().find(|s| s.id == *id)
    }
}

/// How one container is getting on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerSequenceContainerStatus {
    /// The profile the container belongs to.
    pub power_profile_id: Id,
    /// The container this is about.
    pub sequence_container_id: Id,
    /// Which alternative was chosen. Absent when none has been.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_sequence_id: Option<Id>,
    /// How long the chosen sequence has been running. Required once it has started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Duration>,
    /// How far it has got.
    pub status: PowerSequenceStatus,
}

/// The task, its window, and every way of performing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerProfileDefinition {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The profile's own identifier, which every instruction refers back to.
    pub id: Id,
    /// The earliest the first sequence may start.
    pub start_time: Timestamp,
    /// The latest the last sequence must be finished.
    pub end_time: Timestamp,
    /// The containers, in chronological order.
    ///
    /// The wire name is plural in an unusual place — `power_sequences_containers` — and
    /// the Rust field does not inherit that.
    #[serde(rename = "power_sequences_containers")]
    pub power_sequence_containers: Vec<PowerSequenceContainer>,
}

impl PowerProfileDefinition {
    /// The container with this id, if the profile has one.
    #[must_use]
    pub fn container(&self, id: &Id) -> Option<&PowerSequenceContainer> {
        self.power_sequence_containers.iter().find(|c| c.id == *id)
    }

    /// Resolve a container and a sequence together, as every instruction must.
    #[must_use]
    pub fn resolve(
        &self,
        container_id: &Id,
        sequence_id: &Id,
    ) -> Option<(&PowerSequenceContainer, &PowerSequence)> {
        let container = self.container(container_id)?;
        let sequence = container.sequence(sequence_id)?;
        Some((container, sequence))
    }
}

/// How every container of a profile is getting on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerProfileStatus {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// One entry per container in the profile — all of them, not only the interesting
    /// ones.
    pub sequence_container_status: Vec<PowerSequenceContainerStatus>,
}

/// The CEM choosing a sequence and a start time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInstruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier.
    pub id: Id,
    /// The profile being scheduled.
    pub power_profile_id: Id,
    /// The container within it.
    pub sequence_container_id: Id,
    /// The alternative being chosen.
    pub power_sequence_id: Id,
    /// When it should start. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
}

/// The CEM pausing a running sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct StartInterruptionInstruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier.
    pub id: Id,
    /// The profile.
    pub power_profile_id: Id,
    /// The container.
    pub sequence_container_id: Id,
    /// The sequence to interrupt. It must be one that says it is interruptible.
    pub power_sequence_id: Id,
    /// When to interrupt. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
}

/// The CEM resuming an interrupted sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct EndInterruptionInstruction {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction's own identifier.
    pub id: Id,
    /// The profile.
    pub power_profile_id: Id,
    /// The container.
    pub sequence_container_id: Id,
    /// The sequence to resume.
    pub power_sequence_id: Id,
    /// When to resume. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether this is an abnormal-condition instruction.
    pub abnormal_condition: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::CommodityQuantity;

    fn sequence(id: &str, seconds: u64) -> PowerSequence {
        PowerSequence {
            id: Id::parse(id).unwrap(),
            elements: alloc::vec![PowerSequenceElement {
                duration: Duration::from_secs(seconds),
                power_values: alloc::vec![PowerForecastValue::expected(
                    2000.0,
                    CommodityQuantity::ElectricPowerL1
                )],
            }],
            is_interruptible: false,
            max_pause_before: None,
            abnormal_condition_only: false,
        }
    }

    #[test]
    fn the_container_plural_is_on_the_wire_but_not_in_rust() {
        let profile = PowerProfileDefinition {
            message_id: Id::new_const("m1"),
            id: Id::new_const("profile1"),
            start_time: Timestamp::UNIX_EPOCH,
            end_time: Timestamp::UNIX_EPOCH,
            power_sequence_containers: alloc::vec![PowerSequenceContainer {
                id: Id::new_const("c1"),
                power_sequences: alloc::vec![sequence("s1", 3600)],
            }],
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("\"power_sequences_containers\""));
        let back: PowerProfileDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, profile);
    }

    #[test]
    fn resolving_an_instruction_needs_both_ids() {
        let profile = PowerProfileDefinition {
            message_id: Id::new_const("m1"),
            id: Id::new_const("profile1"),
            start_time: Timestamp::UNIX_EPOCH,
            end_time: Timestamp::UNIX_EPOCH,
            power_sequence_containers: alloc::vec![PowerSequenceContainer {
                id: Id::new_const("c1"),
                power_sequences: alloc::vec![sequence("s1", 3600), sequence("s2", 7200)],
            }],
        };
        let (container, seq) = profile
            .resolve(&Id::new_const("c1"), &Id::new_const("s2"))
            .unwrap();
        assert_eq!(container.id, "c1");
        assert_eq!(seq.total_duration(), Duration::from_secs(7200));
        // A sequence from the wrong container does not resolve, which is what stops a
        // Resource Manager starting a wash nobody asked for.
        assert!(
            profile
                .resolve(&Id::new_const("c9"), &Id::new_const("s1"))
                .is_none()
        );
        assert!(
            profile
                .resolve(&Id::new_const("c1"), &Id::new_const("s9"))
                .is_none()
        );
    }

    #[test]
    fn progress_is_required_exactly_once_a_sequence_has_started() {
        assert!(!PowerSequenceStatus::NotScheduled.has_started());
        assert!(!PowerSequenceStatus::Scheduled.has_started());
        assert!(PowerSequenceStatus::Executing.has_started());
        assert!(PowerSequenceStatus::Interrupted.has_started());
        assert!(PowerSequenceStatus::Finished.has_started());
        assert!(PowerSequenceStatus::Aborted.has_started());
        assert!(!PowerSequenceStatus::NotScheduled.has_selection());
        assert!(PowerSequenceStatus::Scheduled.has_selection());
    }
}
