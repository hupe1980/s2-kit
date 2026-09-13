//! The virtual device: what an instruction actually *means*.
//!
//! S2's three state-machine control types describe an abstract device, and an
//! instruction names a state and a number between zero and one. Turning that into watts
//! needs the description that was sent — the S2 documentation, *Operation modes* gives the
//! arithmetic: `power = (end − start) × factor + start`.
//!
//! Both roles need exactly the same arithmetic, and they must not disagree about it. A
//! Resource Manager reads an instruction and works out what its hardware should do; a
//! Customer Energy Manager works out what to ask for, and later reconciles the
//! measurement it gets back. Two implementations of one formula is how those two stop
//! agreeing, so there is one here and both sides call it.
//!
//! Everything in this module is a pure function of a description, an instruction and —
//! where a rate depends on it — a fill level. Nothing reads a clock.
//!
//! ```
//! use s2_kit::prelude::*;
//! use s2_kit::model;
//!
//! // The heat pump from docs `learn/examples/heat-pump`: 500 W to 2000 W.
//! let range = PowerRange::new(500.0, 2000.0, CommodityQuantity::ElectricPowerL1);
//! assert_eq!(model::interpolate(&[range], 0.0)[0].value, 500.0);
//! assert_eq!(model::interpolate(&[range], 1.0)[0].value, 2000.0);
//! assert_eq!(model::interpolate(&[range], 0.5)[0].value, 1250.0);
//! ```

pub mod explain;

use alloc::vec::Vec;

use crate::types::common::{
    CommodityQuantity, NumberRange, PowerRange, PowerValue, Timer, Transition,
};
use crate::types::{Duration, Id, Timestamp, ddbc, frbc, ombc, pebc, ppbc};
use crate::validate::TimerState;

pub use explain::{Explanation, explain};

/// An operation-mode factor: a number in `[0, 1]`.
///
/// The wire type is a bare `f64`, because a message carrying 1.3 has to be
/// representable so the validator can name it. This is the checked form the arithmetic
/// uses.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Factor(f64);

impl Factor {
    /// Fully at the start of every range.
    pub const ZERO: Self = Self(0.0);
    /// Fully at the end of every range.
    pub const ONE: Self = Self(1.0);

    /// A factor, if `value` is within `[0, 1]`.
    #[must_use]
    pub fn try_new(value: f64) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(Self(value))
    }

    /// A factor, clamping anything outside `[0, 1]`.
    ///
    /// For a Resource Manager that has decided to be generous with a peer that is
    /// slightly out of range. The validator will still have reported it.
    #[must_use]
    pub fn clamping(value: f64) -> Self {
        if value.is_nan() {
            Self(0.0)
        } else {
            Self(value.clamp(0.0, 1.0))
        }
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// A ceiling and a floor, as a `PEBC.Instruction` sets them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The most the resource may consume (or the least it may produce).
    pub upper: f64,
    /// The least the resource may consume (or the most it may produce).
    pub lower: f64,
}

/// Why an instruction could not be read against the description that was sent.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveError {
    /// The instruction names an actuator the description does not have.
    #[error("{0} is not an actuator of this system")]
    UnknownActuator(Id),
    /// The instruction names an operation mode the actuator does not have.
    #[error("{0} is not an operation mode of that actuator")]
    UnknownOperationMode(Id),
    /// The instruction names a profile, container or sequence that was never published.
    #[error("{0} was never published")]
    UnknownSequence(Id),
    /// A set of choices does not name each of the profile's containers exactly once, in
    /// the order the profile lists them.
    ///
    /// Array order **is** execution order
    /// (`S2J messages/PPBC.PowerProfileDefinition.power_sequences_containers`), so a set
    /// of choices that skips a container, repeats one or reorders them does not describe
    /// a runnable schedule — and silently scheduling it in the order it was written would
    /// answer a question nobody asked.
    #[error("choice {position} names {found:?}, but the profile's container there is {expected}")]
    WrongContainer {
        /// Where in the list the mismatch is.
        position: usize,
        /// The container the profile lists at that position.
        expected: Id,
        /// What was chosen there, if anything.
        found: Option<Id>,
    },
    /// The factor is outside `[0, 1]`.
    #[error("operation mode factor {0} is outside [0, 1]")]
    BadFactor(f64),
    /// No element of the operation mode covers the present fill level.
    #[error("no element of this operation mode covers a fill level of {0}")]
    FillLevelNotCovered(f64),
    /// No system description for this control type has been seen on this session.
    ///
    /// Not a defect in the instruction: an instruction that arrives before the
    /// description it refers to cannot be read, and saying so is different from saying
    /// the instruction is wrong.
    #[error("no system description for this control type has been seen")]
    NotDescribed,
}

/// The power a set of ranges implies at a factor.
///
/// One value per commodity quantity, in the order the ranges were given.
#[must_use]
pub fn interpolate(ranges: &[PowerRange], factor: f64) -> Vec<PowerValue> {
    ranges
        .iter()
        .map(|r| PowerValue::new(r.commodity_quantity, r.at_factor(factor)))
        .collect()
}

/// The factor that produces `power` for `quantity`, if the ranges determine one.
#[must_use]
pub fn factor_for_power(
    ranges: &[PowerRange],
    quantity: CommodityQuantity,
    power: f64,
) -> Option<f64> {
    ranges
        .iter()
        .find(|r| r.commodity_quantity == quantity)
        .and_then(|r| r.range().factor_of(power))
}

/// Which timers block a move from `from` to `to`, given what is known about them.
///
/// A timer nobody has reported on does **not** block: `S2J messages/*.TimerStatus`
/// says an unstarted timer carries "an arbitrary DateTimeStamp in the past", so silence
/// tells us nothing, and refusing on silence would stall a session on a timer that was
/// never started.
/// `actuator` scopes the lookup: timer identifiers are unique only within the actuator
/// description that declares them, so `Id::NIL` is OMBC's stand-in for "the system
/// description itself".
#[must_use]
pub fn blocking_timers(
    transitions: &[Transition],
    from: &Id,
    to: &Id,
    actuator: &Id,
    known: &[TimerState],
    now: Timestamp,
) -> Vec<Id> {
    transitions
        .iter()
        .find(|t| t.from == *from && t.to == *to)
        .map(|t| {
            t.blocking_timers
                .iter()
                .filter(|id| {
                    known.iter().any(|state| {
                        state.actuator == *actuator
                            && state.timer == **id
                            && state.finished_at > now
                    })
                })
                .copied()
                .collect()
        })
        .unwrap_or_default()
}

/// How long a transition takes to take effect. Absent means negligible.
#[must_use]
pub fn transition_duration(transitions: &[Transition], from: &Id, to: &Id) -> Duration {
    transitions
        .iter()
        .find(|t| t.from == *from && t.to == *to)
        .and_then(|t| t.transition_duration)
        .unwrap_or(Duration::ZERO)
}

/// The timer with this identifier, for a diagnostic.
#[must_use]
pub fn timer_label<'a>(timers: &'a [Timer], id: &Id) -> Option<&'a str> {
    timers
        .iter()
        .find(|t| t.id == *id)
        .and_then(|t| t.diagnostic_label.as_deref())
}

// ---------------------------------------------------------------------------
// Fill Rate Based Control
// ---------------------------------------------------------------------------

/// What an `FRBC.Instruction` means, read against the system description.
#[derive(Debug, Clone, PartialEq)]
pub struct FrbcResolution<'a> {
    /// The actuator it addresses.
    pub actuator: &'a frbc::ActuatorDescription,
    /// The operation mode it selects.
    pub mode: &'a frbc::OperationMode,
    /// The element of that mode which applies at the present fill level, when one is
    /// known.
    pub element: Option<&'a frbc::OperationModeElement>,
    /// The factor, checked.
    pub factor: Factor,
    /// The power that factor implies, one value per commodity quantity.
    pub power: Vec<PowerValue>,
    /// How fast the store fills, in fill-level units per second. Positive fills.
    pub fill_rate: Option<f64>,
    /// Whether the mode may only be used during an abnormal condition.
    pub abnormal_only: bool,
}

/// Read an `FRBC.Instruction` against the description that was sent.
///
/// `fill_level` selects which element of the operation mode applies; without it the
/// power is still resolved from the first element, because every element of a mode
/// covers the same commodity quantities, but `fill_rate` is left unanswered.
pub fn resolve_frbc<'a>(
    system: &'a frbc::SystemDescription,
    instruction: &frbc::Instruction,
    fill_level: Option<f64>,
) -> Result<FrbcResolution<'a>, ResolveError> {
    let actuator = system
        .actuator(&instruction.actuator_id)
        .ok_or(ResolveError::UnknownActuator(instruction.actuator_id))?;
    let mode = actuator.operation_mode(&instruction.operation_mode).ok_or(
        ResolveError::UnknownOperationMode(instruction.operation_mode),
    )?;
    let factor = Factor::try_new(instruction.operation_mode_factor)
        .ok_or(ResolveError::BadFactor(instruction.operation_mode_factor))?;

    let element = match fill_level {
        Some(level) => Some(
            mode.element_at(level)
                .ok_or(ResolveError::FillLevelNotCovered(level))?,
        ),
        None => mode.elements.first(),
    };

    let power = element.map(|e| interpolate(&e.power_ranges, factor.get()));
    Ok(FrbcResolution {
        actuator,
        mode,
        element,
        factor,
        power: power.unwrap_or_default(),
        fill_rate: element.map(|e| e.fill_rate.at_factor(factor.get())),
        abnormal_only: mode.abnormal_condition_only,
    })
}

/// Where the fill level ends up after `duration` in one operation mode.
///
/// Integrates the mode's fill rate, the storage's leakage and the user's expected usage
/// over one step, using the element that applies at each moment. Leakage and usage are
/// *positive when the level falls*, which is the standard's convention and the one thing
/// about FRBC that is easiest to get backwards.
///
/// Deliberately a simple forward integration: the caller chooses the step, and a CEM
/// that wants more accuracy calls it with smaller ones.
#[must_use]
pub fn project_fill_level(
    mode: &frbc::OperationMode,
    factor: Factor,
    from: f64,
    duration: Duration,
    leakage: Option<&[frbc::LeakageBehaviourElement]>,
    usage_rate: Option<f64>,
) -> f64 {
    let seconds = duration.as_secs_f64();
    if seconds <= 0.0 {
        return from;
    }
    let fill_rate = mode
        .element_at(from)
        .map_or(0.0, |e| e.fill_rate.at_factor(factor.get()));
    let leak = leakage
        .and_then(|elements| {
            elements
                .iter()
                .find(|e| e.fill_level_range.contains(from))
                .map(|e| e.leakage_rate)
        })
        .unwrap_or(0.0);
    let usage = usage_rate.unwrap_or(0.0);
    from + (fill_rate - leak - usage) * seconds
}

// ---------------------------------------------------------------------------
// Operation Mode Based Control
// ---------------------------------------------------------------------------

/// What an `OMBC.Instruction` means.
#[derive(Debug, Clone, PartialEq)]
pub struct OmbcResolution<'a> {
    /// The mode it selects.
    pub mode: &'a ombc::OperationMode,
    /// The factor, checked.
    pub factor: Factor,
    /// The power that factor implies.
    pub power: Vec<PowerValue>,
    /// Whether the mode may only be used during an abnormal condition.
    pub abnormal_only: bool,
}

/// Read an `OMBC.Instruction` against the description that was sent.
pub fn resolve_ombc<'a>(
    system: &'a ombc::SystemDescription,
    instruction: &ombc::Instruction,
) -> Result<OmbcResolution<'a>, ResolveError> {
    let mode = system
        .operation_mode(&instruction.operation_mode_id)
        .ok_or(ResolveError::UnknownOperationMode(
            instruction.operation_mode_id,
        ))?;
    let factor = Factor::try_new(instruction.operation_mode_factor)
        .ok_or(ResolveError::BadFactor(instruction.operation_mode_factor))?;
    Ok(OmbcResolution {
        mode,
        factor,
        power: interpolate(&mode.power_ranges, factor.get()),
        abnormal_only: mode.abnormal_condition_only,
    })
}

// ---------------------------------------------------------------------------
// Demand Driven Based Control
// ---------------------------------------------------------------------------

/// What a `DDBC.Instruction` means.
#[derive(Debug, Clone, PartialEq)]
pub struct DdbcResolution<'a> {
    /// The actuator it addresses.
    pub actuator: &'a ddbc::ActuatorDescription,
    /// The mode it selects.
    pub mode: &'a ddbc::OperationMode,
    /// The factor, checked.
    pub factor: Factor,
    /// The power that factor implies.
    pub power: Vec<PowerValue>,
    /// The supply rate the mode delivers at that factor — what the CEM matches against
    /// the demand.
    pub supply_rate: f64,
    /// Whether the mode may only be used during an abnormal condition.
    pub abnormal_only: bool,
}

/// Read a `DDBC.Instruction` against the description that was sent.
pub fn resolve_ddbc<'a>(
    system: &'a ddbc::SystemDescription,
    instruction: &ddbc::Instruction,
) -> Result<DdbcResolution<'a>, ResolveError> {
    let actuator = system
        .actuator(&instruction.actuator_id)
        .ok_or(ResolveError::UnknownActuator(instruction.actuator_id))?;
    let mode = actuator
        .operation_mode(&instruction.operation_mode_id)
        .ok_or(ResolveError::UnknownOperationMode(
            instruction.operation_mode_id,
        ))?;
    let factor = Factor::try_new(instruction.operation_mode_factor)
        .ok_or(ResolveError::BadFactor(instruction.operation_mode_factor))?;
    Ok(DdbcResolution {
        actuator,
        mode,
        factor,
        power: interpolate(&mode.power_ranges, factor.get()),
        supply_rate: mode.supply_range.at_factor(factor.get()),
        abnormal_only: mode.abnormal_condition_only,
    })
}

/// The whole span of demand one actuator can supply, across all of its operation modes.
///
/// The union of every mode's `supply_range`, so a CEM driving a hybrid system can ask
/// "could this actuator meet the demand at all?" before working out which mode does it.
/// An actuator with no operation modes answers `0..0`.
#[must_use]
pub fn supply_range_of(actuator: &ddbc::ActuatorDescription) -> NumberRange {
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for mode in &actuator.operation_modes {
        let (a, b) = mode.supply_range.ordered();
        low = low.min(a);
        high = high.max(b);
    }
    if low.is_finite() && high.is_finite() {
        NumberRange::new(low, high)
    } else {
        NumberRange::new(0.0, 0.0)
    }
}

// ---------------------------------------------------------------------------
// Power Envelope Based Control
// ---------------------------------------------------------------------------

/// The limits a `PEBC.Instruction` puts on one quantity at a moment.
///
/// `None` when the instruction says nothing about that quantity at that moment — either
/// it carries no envelope for it, or the envelope has run out. An envelope that has run
/// out leaves the resource unbounded again, which is a different thing from being held
/// at its last step for ever.
#[must_use]
pub fn limits_at(
    instruction: &pebc::Instruction,
    quantity: CommodityQuantity,
    at: Timestamp,
) -> Option<Limits> {
    let envelope = instruction.envelope_for(quantity)?;
    // `S2J messages/PEBC.Instruction.execution_time`: in the past means "as soon as
    // possible", so an envelope whose execution time has passed is measured from then.
    let offset = at.checked_duration_since(instruction.execution_time)?;
    envelope.element_at(offset).map(|e| Limits {
        upper: e.upper_limit,
        lower: e.lower_limit,
    })
}

/// Whether a power obeys the limits.
#[must_use]
pub fn within(limits: Limits, power: f64) -> bool {
    power >= limits.lower && power <= limits.upper
}

/// The envelope the resource is under for each quantity the instruction bounds.
#[must_use]
pub fn all_limits_at(
    instruction: &pebc::Instruction,
    at: Timestamp,
) -> Vec<(CommodityQuantity, Limits)> {
    instruction
        .power_envelopes
        .iter()
        .filter_map(|e| {
            limits_at(instruction, e.commodity_quantity, at).map(|l| (e.commodity_quantity, l))
        })
        .collect()
}

/// The average power an energy constraint demands over its window, as a range.
#[must_use]
pub fn energy_window(constraint: &pebc::EnergyConstraint) -> Option<(Duration, NumberRange)> {
    let window = constraint
        .valid_until
        .checked_duration_since(constraint.valid_from)?;
    Some((
        window,
        NumberRange::new(
            constraint.lower_average_power,
            constraint.upper_average_power,
        ),
    ))
}

// ---------------------------------------------------------------------------
// Power Profile Based Control
// ---------------------------------------------------------------------------

/// What a PPBC instruction means.
#[derive(Debug, Clone, PartialEq)]
pub struct PpbcResolution<'a> {
    /// The profile.
    pub profile: &'a ppbc::PowerProfileDefinition,
    /// The container within it.
    pub container: &'a ppbc::PowerSequenceContainer,
    /// The sequence that was chosen.
    pub sequence: &'a ppbc::PowerSequence,
    /// How long that sequence takes.
    pub duration: Duration,
}

/// Read a PPBC instruction's three identifiers against the profile that was published.
pub fn resolve_ppbc<'a>(
    profiles: &'a [ppbc::PowerProfileDefinition],
    profile_id: &Id,
    container_id: &Id,
    sequence_id: &Id,
) -> Result<PpbcResolution<'a>, ResolveError> {
    let profile = profiles
        .iter()
        .find(|p| p.id == *profile_id)
        .ok_or(ResolveError::UnknownSequence(*profile_id))?;
    let (container, sequence) = profile
        .resolve(container_id, sequence_id)
        .ok_or(ResolveError::UnknownSequence(*sequence_id))?;
    Ok(PpbcResolution {
        profile,
        container,
        sequence,
        duration: sequence.total_duration(),
    })
}

/// The power a sequence draws `offset` after it started.
#[must_use]
pub fn sequence_power_at(
    sequence: &ppbc::PowerSequence,
    offset: Duration,
) -> Option<&[crate::types::common::PowerForecastValue]> {
    let mut elapsed = 0u64;
    let target = offset.as_millis();
    for element in &sequence.elements {
        let end = elapsed.saturating_add(element.duration.as_millis());
        if target < end {
            return Some(&element.power_values);
        }
        elapsed = end;
    }
    None
}

/// Where one chosen sequence has to fall for the whole profile to fit its window.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceWindow {
    /// The container the choice was made in.
    pub container: Id,
    /// The sequence chosen from it.
    pub sequence: Id,
    /// The earliest it may start: the profile opens, or its predecessor finishes.
    pub earliest_start: Timestamp,
    /// The latest it may start and still leave room for every sequence after it.
    pub latest_start: Timestamp,
    /// How long it runs.
    pub duration: Duration,
    /// The longest it may idle after its predecessor finished, when the sequence says.
    ///
    /// This is a constraint *between* two starts, not a bound on this one, so it cannot
    /// be folded into `latest_start` — see [`schedule`]. [`starts_are_feasible`] is what
    /// applies it.
    pub max_pause_before: Option<Duration>,
}

impl SequenceWindow {
    /// Whether there is any start time at all that works.
    #[must_use]
    pub fn is_feasible(&self) -> bool {
        self.earliest_start <= self.latest_start
    }
}

/// When each chosen sequence may run, given that containers run in the order the profile
/// lists them.
///
/// `S2J messages/PPBC.PowerProfileDefinition.power_sequences_containers`:
/// "PPBC.PowerSequenceContainers must be placed in chronological order." A container has
/// no timestamp, so that is an interpretation rather than a checkable rule: container 0's
/// chosen sequence runs, then container 1's, all inside `start_time .. end_time`. The
/// latest a sequence may start is therefore `end_time` minus everything still to come,
/// not minus its own duration (D32).
///
/// [`max_pause_before`](crate::types::ppbc::PowerSequence::max_pause_before) is carried on
/// each window but not folded into it: it bounds the gap between two starts, so it can
/// only reject a schedule. [`starts_are_feasible`] applies it.
///
/// `chosen` names one sequence per container, **in the order the profile lists the
/// containers**; every container must appear exactly once. A list that skips, repeats or
/// reorders a container is [`ResolveError::WrongContainer`] rather than a schedule for a
/// profile nobody published.
pub fn schedule(
    profile: &ppbc::PowerProfileDefinition,
    chosen: &[(Id, Id)],
) -> Result<Vec<SequenceWindow>, ResolveError> {
    // Position by position against the profile's own order, so a skipped container, a
    // repeated one and a swapped pair are all caught, and all name the container the
    // profile expected there.
    for (position, container) in profile.power_sequence_containers.iter().enumerate() {
        let found = chosen.get(position).map(|(id, _)| *id);
        if found != Some(container.id) {
            return Err(ResolveError::WrongContainer {
                position,
                expected: container.id,
                found,
            });
        }
    }
    if chosen.len() > profile.power_sequence_containers.len() {
        let position = profile.power_sequence_containers.len();
        return Err(ResolveError::WrongContainer {
            position,
            expected: profile.id,
            found: chosen.get(position).map(|(id, _)| *id),
        });
    }

    let mut resolved = Vec::with_capacity(chosen.len());
    for (container_id, sequence_id) in chosen {
        let (container, sequence) = profile
            .resolve(container_id, sequence_id)
            .ok_or(ResolveError::UnknownSequence(*sequence_id))?;
        resolved.push((container, sequence));
    }

    // Backwards: how much time everything from `k` onwards needs.
    let mut tail = Vec::with_capacity(resolved.len());
    let mut running = Duration::ZERO;
    for (_, sequence) in resolved.iter().rev() {
        running = running
            .checked_add(sequence.total_duration())
            .ok_or(ResolveError::UnknownSequence(profile.id))?;
        tail.push(running);
    }
    tail.reverse();

    let mut out: Vec<SequenceWindow> = Vec::with_capacity(resolved.len());
    for (index, (container, sequence)) in resolved.iter().enumerate() {
        let duration = sequence.total_duration();
        let needed = tail.get(index).copied().unwrap_or(duration);
        let earliest_start = match out.last() {
            None => profile.start_time,
            Some(previous) => previous
                .earliest_start
                .checked_add(previous.duration)
                .unwrap_or(previous.earliest_start),
        };
        let latest_start = profile
            .end_time
            .checked_sub(needed)
            .unwrap_or(profile.start_time);

        out.push(SequenceWindow {
            container: container.id,
            sequence: sequence.id,
            earliest_start,
            latest_start,
            duration,
            max_pause_before: sequence.max_pause_before,
        });
    }
    Ok(out)
}

/// Whether a set of choices can be run inside the profile's window at all.
///
/// The question a CEM asks before it commits to a `PPBC.ScheduleInstruction`.
#[must_use]
pub fn is_schedulable(profile: &ppbc::PowerProfileDefinition, chosen: &[(Id, Id)]) -> bool {
    schedule(profile, chosen).is_ok_and(|w| w.iter().all(SequenceWindow::is_feasible))
}

/// Why a concrete set of start times will not do.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScheduleProblem {
    /// The choices do not describe the profile at all, so there is nothing to time.
    ///
    /// Kept distinct from [`WrongCount`](Self::WrongCount): "you named a sequence that
    /// does not exist" and "you gave me four start times for three sequences" are
    /// different mistakes, and collapsing them reports the second for the first.
    #[error("{0}")]
    NotSchedulable(ResolveError),
    /// One start time per chosen sequence is needed, and a different number was given.
    #[error("{given} start times for {needed} sequences")]
    WrongCount {
        /// How many were given.
        given: usize,
        /// How many were needed.
        needed: usize,
    },
    /// A sequence starts before the profile opens, or before its predecessor finishes.
    #[error("sequence {sequence} starts at {at}, before {not_before}")]
    TooEarly {
        /// Which sequence.
        sequence: Id,
        /// When it was asked to start.
        at: Timestamp,
        /// The earliest it may.
        not_before: Timestamp,
    },
    /// A sequence would still be running when the profile's window closes.
    #[error("sequence {sequence} starts at {at} and would finish after {end_time}")]
    Overruns {
        /// Which sequence.
        sequence: Id,
        /// When it was asked to start.
        at: Timestamp,
        /// When the window closes.
        end_time: Timestamp,
    },
    /// The device would idle longer than the sequence allows.
    ///
    /// `S2J schemas/PPBC.PowerSequence.max_pause_before`.
    #[error("sequence {sequence} idles {idle} after the previous one, but allows only {allowed}")]
    PauseTooLong {
        /// Which sequence.
        sequence: Id,
        /// How long the gap is.
        idle: Duration,
        /// How long it may be.
        allowed: Duration,
    },
}

/// Whether concrete start times run the profile as the standard says it must be run.
///
/// Checks what [`schedule`] cannot: that the sequences are in order, that none overlaps
/// its predecessor, that the last one finishes inside the window, and that no gap exceeds
/// the following sequence's `max_pause_before`. `starts` is one instant per entry of
/// `chosen`, in the same order.
pub fn starts_are_feasible(
    profile: &ppbc::PowerProfileDefinition,
    chosen: &[(Id, Id)],
    starts: &[Timestamp],
) -> Result<(), ScheduleProblem> {
    let windows = schedule(profile, chosen).map_err(ScheduleProblem::NotSchedulable)?;
    if starts.len() != windows.len() {
        return Err(ScheduleProblem::WrongCount {
            given: starts.len(),
            needed: windows.len(),
        });
    }

    let mut previous_end: Option<Timestamp> = None;
    for (window, start) in windows.iter().zip(starts) {
        let not_before = previous_end.unwrap_or(profile.start_time);
        if *start < not_before {
            return Err(ScheduleProblem::TooEarly {
                sequence: window.sequence,
                at: *start,
                not_before,
            });
        }
        if let (Some(end), Some(allowed)) = (previous_end, window.max_pause_before) {
            let idle = start.saturating_duration_since(end);
            if idle > allowed {
                return Err(ScheduleProblem::PauseTooLong {
                    sequence: window.sequence,
                    idle,
                    allowed,
                });
            }
        }
        let end = start
            .checked_add(window.duration)
            .unwrap_or(profile.end_time);
        if end > profile.end_time {
            return Err(ScheduleProblem::Overruns {
                sequence: window.sequence,
                at: *start,
                end_time: profile.end_time,
            });
        }
        previous_end = Some(end);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::Commodity;
    use alloc::string::ToString;
    use alloc::vec;

    fn charging_mode() -> frbc::OperationMode {
        // docs `learn/examples/ev`: 1.4 kW to 11 kW, filling at 0.00065 to 0.0051 %/s.
        frbc::OperationMode {
            id: Id::new_const("om2"),
            diagnostic_label: None,
            elements: vec![frbc::OperationModeElement {
                fill_level_range: NumberRange::new(0.0, 100.0),
                fill_rate: NumberRange::new(0.000_65, 0.0051),
                power_ranges: vec![PowerRange::new(
                    1400.0,
                    11_000.0,
                    CommodityQuantity::ElectricPower3PhaseSymmetric,
                )],
                running_costs: None,
            }],
            abnormal_condition_only: false,
        }
    }

    fn ev_system() -> frbc::SystemDescription {
        frbc::SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: Timestamp::UNIX_EPOCH,
            actuators: vec![frbc::ActuatorDescription {
                id: Id::new_const("actuator1"),
                diagnostic_label: None,
                supported_commodities: vec![Commodity::Electricity],
                operation_modes: vec![
                    frbc::OperationMode {
                        id: Id::new_const("om1"),
                        diagnostic_label: None,
                        elements: vec![frbc::OperationModeElement {
                            fill_level_range: NumberRange::new(0.0, 100.0),
                            fill_rate: NumberRange::exactly(0.0),
                            power_ranges: vec![PowerRange::exactly(
                                0.0,
                                CommodityQuantity::ElectricPower3PhaseSymmetric,
                            )],
                            running_costs: None,
                        }],
                        abnormal_condition_only: false,
                    },
                    charging_mode(),
                ],
                transitions: vec![
                    Transition::simple(
                        Id::new_const("transition1"),
                        Id::new_const("om1"),
                        Id::new_const("om2"),
                    ),
                    Transition::simple(
                        Id::new_const("transition2"),
                        Id::new_const("om2"),
                        Id::new_const("om1"),
                    ),
                ],
                timers: vec![],
            }],
            storage: frbc::StorageDescription {
                diagnostic_label: None,
                fill_level_label: Some("EV Battery SoC".into()),
                provides_leakage_behaviour: false,
                provides_fill_level_target_profile: true,
                provides_usage_forecast: false,
                fill_level_range: NumberRange::new(0.0, 100.0),
            },
        }
    }

    fn charge_at(factor: f64) -> frbc::Instruction {
        frbc::Instruction {
            message_id: Id::new_const("m2"),
            id: Id::new_const("instr0"),
            actuator_id: Id::new_const("actuator1"),
            operation_mode: Id::new_const("om2"),
            operation_mode_factor: factor,
            execution_time: Timestamp::UNIX_EPOCH,
            abnormal_condition: false,
        }
    }

    #[test]
    fn an_instruction_resolves_to_the_power_the_documentation_says() {
        let system = ev_system();
        let resolved = resolve_frbc(&system, &charge_at(0.0), Some(50.0)).unwrap();
        assert_eq!(resolved.mode.id, "om2");
        assert!((resolved.power[0].value - 1400.0).abs() < 1e-9);
        assert!((resolved.fill_rate.unwrap() - 0.000_65).abs() < 1e-12);

        let resolved = resolve_frbc(&system, &charge_at(1.0), Some(50.0)).unwrap();
        assert!((resolved.power[0].value - 11_000.0).abs() < 1e-9);
        assert!((resolved.fill_rate.unwrap() - 0.0051).abs() < 1e-12);

        // Halfway is halfway, in both quantities.
        let resolved = resolve_frbc(&system, &charge_at(0.5), Some(50.0)).unwrap();
        assert!((resolved.power[0].value - 6200.0).abs() < 1e-9);
    }

    #[test]
    fn resolution_refuses_what_was_never_described() {
        let system = ev_system();
        let mut bad = charge_at(0.5);
        bad.actuator_id = Id::new_const("nope");
        assert_eq!(
            resolve_frbc(&system, &bad, None),
            Err(ResolveError::UnknownActuator(Id::new_const("nope")))
        );

        let mut bad = charge_at(0.5);
        bad.operation_mode = Id::new_const("om9");
        assert_eq!(
            resolve_frbc(&system, &bad, None),
            Err(ResolveError::UnknownOperationMode(Id::new_const("om9")))
        );

        assert_eq!(
            resolve_frbc(&system, &charge_at(1.3), None),
            Err(ResolveError::BadFactor(1.3))
        );

        // A fill level no element covers is a description that does not answer the
        // question, not a fill rate of zero.
        assert_eq!(
            resolve_frbc(&system, &charge_at(0.5), Some(150.0)),
            Err(ResolveError::FillLevelNotCovered(150.0))
        );
    }

    #[test]
    fn the_factor_and_the_power_are_inverses() {
        let system = ev_system();
        let resolved = resolve_frbc(&system, &charge_at(0.37), Some(10.0)).unwrap();
        let back = factor_for_power(
            &resolved.element.unwrap().power_ranges,
            CommodityQuantity::ElectricPower3PhaseSymmetric,
            resolved.power[0].value,
        )
        .unwrap();
        assert!((back - 0.37).abs() < 1e-12);
    }

    #[test]
    fn projection_uses_the_element_that_applies_and_the_right_signs() {
        let mode = charging_mode();
        // An hour at full power: 0.0051 %/s * 3600 s = 18.36 %.
        let after = project_fill_level(
            &mode,
            Factor::ONE,
            50.0,
            Duration::from_secs(3600),
            None,
            None,
        );
        assert!((after - 68.36).abs() < 1e-9, "{after}");

        // Leakage and usage are positive when the level *falls*.
        let leakage = [frbc::LeakageBehaviourElement {
            fill_level_range: NumberRange::new(0.0, 100.0),
            leakage_rate: 0.001,
        }];
        let after = project_fill_level(
            &mode,
            Factor::ZERO,
            50.0,
            Duration::from_secs(1000),
            Some(&leakage),
            Some(0.002),
        );
        // (0.00065 - 0.001 - 0.002) * 1000 = -2.35
        assert!((after - 47.65).abs() < 1e-9, "{after}");
    }

    #[test]
    fn a_timer_nobody_reported_on_does_not_block() {
        let transitions = vec![Transition {
            id: Id::new_const("t1"),
            from: Id::new_const("om1"),
            to: Id::new_const("om2"),
            start_timers: vec![],
            blocking_timers: vec![Id::new_const("timer0")],
            transition_costs: None,
            transition_duration: None,
            abnormal_condition_only: false,
        }];
        let now: Timestamp = "2024-01-01T12:00:00Z".parse().unwrap();

        let actuator = Id::new_const("actuator1");
        let state = |actuator: Id, at: &str| TimerState {
            actuator,
            timer: Id::new_const("timer0"),
            finished_at: at.parse().unwrap(),
        };
        let blocked = |known: &[TimerState]| {
            blocking_timers(
                &transitions,
                &Id::new_const("om1"),
                &Id::new_const("om2"),
                &actuator,
                known,
                now,
            )
        };

        // Nothing known about the timer: not blocked.
        assert!(blocked(&[]).is_empty());

        // Finished in the past: not blocked.
        assert!(blocked(&[state(actuator, "2024-01-01T11:00:00Z")]).is_empty());

        // Finishes later: blocked.
        assert_eq!(
            blocked(&[state(actuator, "2024-01-01T13:00:00Z")]),
            vec![Id::new_const("timer0")]
        );

        // The same timer identifier, running, but on a *different* actuator. Timer ids
        // are unique only within the actuator description that declares them, so this
        // must not block: `S2J schemas/Timer.id`.
        assert!(blocked(&[state(Id::new_const("actuator2"), "2024-01-01T13:00:00Z")]).is_empty());
    }

    #[test]
    fn an_envelope_bounds_only_while_it_lasts() {
        let start: Timestamp = "2024-08-24T15:00:00Z".parse().unwrap();
        let instruction = pebc::Instruction {
            message_id: Id::new_const("m1"),
            id: Id::new_const("envelope1"),
            execution_time: start,
            abnormal_condition: false,
            power_constraints_id: Id::new_const("powerConstraint1"),
            power_envelopes: vec![pebc::PowerEnvelope {
                id: Id::new_const("pe_xxx"),
                commodity_quantity: CommodityQuantity::ElectricPowerL1,
                power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                    Duration::from_secs(3600),
                    -2000.0,
                    0.0,
                )],
            }],
        };

        let during = limits_at(
            &instruction,
            CommodityQuantity::ElectricPowerL1,
            "2024-08-24T15:30:00Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(
            during,
            Limits {
                upper: 0.0,
                lower: -2000.0
            }
        );
        assert!(within(during, -1500.0));
        assert!(!within(during, -2500.0));

        // Before it starts and after it ends, the resource is unbounded.
        assert!(
            limits_at(
                &instruction,
                CommodityQuantity::ElectricPowerL1,
                "2024-08-24T14:59:59Z".parse().unwrap()
            )
            .is_none()
        );
        assert!(
            limits_at(
                &instruction,
                CommodityQuantity::ElectricPowerL1,
                "2024-08-24T16:00:00Z".parse().unwrap()
            )
            .is_none()
        );
        // And it says nothing about a quantity it does not carry.
        assert!(
            limits_at(
                &instruction,
                CommodityQuantity::ElectricPowerL2,
                "2024-08-24T15:30:00Z".parse().unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn a_sequence_knows_its_own_shape() {
        let sequence = ppbc::PowerSequence {
            id: Id::new_const("s1"),
            elements: vec![
                ppbc::PowerSequenceElement {
                    duration: Duration::from_secs(600),
                    power_values: vec![crate::types::common::PowerForecastValue::expected(
                        2000.0,
                        CommodityQuantity::ElectricPowerL1,
                    )],
                },
                ppbc::PowerSequenceElement {
                    duration: Duration::from_secs(1800),
                    power_values: vec![crate::types::common::PowerForecastValue::expected(
                        500.0,
                        CommodityQuantity::ElectricPowerL1,
                    )],
                },
            ],
            is_interruptible: true,
            max_pause_before: None,
            abnormal_condition_only: false,
        };
        assert_eq!(sequence.total_duration(), Duration::from_secs(2400));
        assert_eq!(
            sequence_power_at(&sequence, Duration::from_secs(0)).unwrap()[0].value_expected,
            2000.0
        );
        assert_eq!(
            sequence_power_at(&sequence, Duration::from_secs(1200)).unwrap()[0].value_expected,
            500.0
        );
        assert!(sequence_power_at(&sequence, Duration::from_secs(2400)).is_none());

        let profile = ppbc::PowerProfileDefinition {
            message_id: Id::new_const("m1"),
            id: Id::new_const("p1"),
            start_time: "2024-01-01T08:00:00Z".parse().unwrap(),
            end_time: "2024-01-01T20:00:00Z".parse().unwrap(),
            power_sequence_containers: vec![ppbc::PowerSequenceContainer {
                id: Id::new_const("c1"),
                power_sequences: vec![sequence.clone()],
            }],
        };
        // One container: the latest start is the window's end less this sequence.
        let windows = schedule(&profile, &[(Id::new_const("c1"), Id::new_const("s1"))]).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0].earliest_start.to_string(),
            "2024-01-01T08:00:00Z"
        );
        assert_eq!(windows[0].latest_start.to_string(), "2024-01-01T19:20:00Z");
        assert!(windows[0].is_feasible());
        let resolved = resolve_ppbc(
            core::slice::from_ref(&profile),
            &Id::new_const("p1"),
            &Id::new_const("c1"),
            &Id::new_const("s1"),
        )
        .unwrap();
        assert_eq!(resolved.duration, Duration::from_secs(2400));
    }

    #[test]
    fn a_profile_of_several_containers_leaves_room_for_the_ones_that_follow() {
        // Three containers, one hour each, in a five-hour window. The obvious "latest
        // start = end_time − my own duration" would let container 0 start at 16:00 and
        // the profile would overrun by two hours. Containers run in the order the
        // profile lists them, so each has to leave room for the rest.
        let hour = Duration::from_secs(3600);
        let step = |id: &'static str, pause: Option<Duration>| ppbc::PowerSequenceContainer {
            id: Id::new_const(id),
            power_sequences: vec![ppbc::PowerSequence {
                id: Id::new_const("sq"),
                elements: vec![ppbc::PowerSequenceElement {
                    duration: hour,
                    power_values: vec![],
                }],
                is_interruptible: false,
                max_pause_before: pause,
                abnormal_condition_only: false,
            }],
        };
        let profile = ppbc::PowerProfileDefinition {
            message_id: Id::new_const("m1"),
            id: Id::new_const("p1"),
            start_time: "2024-01-01T12:00:00Z".parse().unwrap(),
            end_time: "2024-01-01T17:00:00Z".parse().unwrap(),
            power_sequence_containers: vec![step("c1", None), step("c2", None), step("c3", None)],
        };
        let chosen = [
            (Id::new_const("c1"), Id::new_const("sq")),
            (Id::new_const("c2"), Id::new_const("sq")),
            (Id::new_const("c3"), Id::new_const("sq")),
        ];

        let windows = schedule(&profile, &chosen).unwrap();
        let at = |i: usize| {
            (
                windows[i].earliest_start.to_string(),
                windows[i].latest_start.to_string(),
            )
        };
        // Three hours of work in a five-hour window: two hours of slack, shared.
        assert_eq!(
            at(0),
            ("2024-01-01T12:00:00Z".into(), "2024-01-01T14:00:00Z".into())
        );
        assert_eq!(
            at(1),
            ("2024-01-01T13:00:00Z".into(), "2024-01-01T15:00:00Z".into())
        );
        assert_eq!(
            at(2),
            ("2024-01-01T14:00:00Z".into(), "2024-01-01T16:00:00Z".into())
        );
        assert!(is_schedulable(&profile, &chosen));

        // A window too short for the whole task is infeasible, and says which step fails.
        let mut tight = profile.clone();
        tight.end_time = "2024-01-01T14:00:00Z".parse().unwrap();
        assert!(!is_schedulable(&tight, &chosen));

        // Concrete start times are where `max_pause_before` bites. It is a bound on the
        // *gap*, so it constrains a schedule rather than a window: back to back is fine,
        // and so is any delay the window allows — until a sequence says otherwise.
        let back_to_back = [
            "2024-01-01T12:00:00Z".parse().unwrap(),
            "2024-01-01T13:00:00Z".parse().unwrap(),
            "2024-01-01T14:00:00Z".parse().unwrap(),
        ];
        assert_eq!(
            starts_are_feasible(&profile, &chosen, &back_to_back),
            Ok(())
        );

        let spread_out = [
            "2024-01-01T12:00:00Z".parse().unwrap(),
            "2024-01-01T14:00:00Z".parse().unwrap(),
            "2024-01-01T15:00:00Z".parse().unwrap(),
        ];
        assert_eq!(starts_are_feasible(&profile, &chosen, &spread_out), Ok(()));

        // The same spread, but now the middle step may idle only fifteen minutes.
        let mut impatient = profile.clone();
        impatient.power_sequence_containers[1] = step("c2", Some(Duration::from_secs(900)));
        assert!(matches!(
            starts_are_feasible(&impatient, &chosen, &spread_out),
            Err(ScheduleProblem::PauseTooLong { idle, allowed, .. })
                if idle == Duration::from_secs(3600) && allowed == Duration::from_secs(900)
        ));
        // Its window is unchanged, because a pause is not a bound on one start.
        assert_eq!(
            schedule(&impatient, &chosen).unwrap()[1].latest_start,
            windows[1].latest_start
        );

        // Overlapping the predecessor, and overrunning the window, are the other two.
        let overlapping = [
            "2024-01-01T12:00:00Z".parse().unwrap(),
            "2024-01-01T12:30:00Z".parse().unwrap(),
            "2024-01-01T14:00:00Z".parse().unwrap(),
        ];
        assert!(matches!(
            starts_are_feasible(&profile, &chosen, &overlapping),
            Err(ScheduleProblem::TooEarly { .. })
        ));
        let too_late = [
            "2024-01-01T13:00:00Z".parse().unwrap(),
            "2024-01-01T15:00:00Z".parse().unwrap(),
            "2024-01-01T16:30:00Z".parse().unwrap(),
        ];
        assert!(matches!(
            starts_are_feasible(&profile, &chosen, &too_late),
            Err(ScheduleProblem::Overruns { .. })
        ));

        // A profile is the whole task: leaving a container unchosen is an error, and the
        // error names the container nobody chose for.
        let partial = [(Id::new_const("c1"), Id::new_const("sq"))];
        assert_eq!(
            schedule(&profile, &partial),
            Err(ResolveError::WrongContainer {
                position: 1,
                expected: Id::new_const("c2"),
                found: None,
            })
        );

        // Array order is execution order, so choices given out of order describe a
        // different schedule from the one the profile published — and a `schedule` that
        // quietly honoured the caller's order would answer a question nobody asked.
        let swapped = [
            (Id::new_const("c2"), Id::new_const("sq")),
            (Id::new_const("c1"), Id::new_const("sq")),
            (Id::new_const("c3"), Id::new_const("sq")),
        ];
        assert_eq!(
            schedule(&profile, &swapped),
            Err(ResolveError::WrongContainer {
                position: 0,
                expected: Id::new_const("c1"),
                found: Some(Id::new_const("c2")),
            })
        );

        // And one container chosen twice is not two containers, however well the count
        // lines up.
        let doubled = [
            (Id::new_const("c1"), Id::new_const("sq")),
            (Id::new_const("c1"), Id::new_const("sq")),
            (Id::new_const("c3"), Id::new_const("sq")),
        ];
        assert!(matches!(
            schedule(&profile, &doubled),
            Err(ResolveError::WrongContainer { position: 1, .. })
        ));

        // A start-time check on choices that do not describe the profile says *that*,
        // rather than blaming the number of start times.
        assert!(matches!(
            starts_are_feasible(&profile, &partial, &back_to_back[..1]),
            Err(ScheduleProblem::NotSchedulable(
                ResolveError::WrongContainer { .. }
            ))
        ));
    }

    #[test]
    fn a_factor_refuses_what_is_out_of_range_and_clamps_on_request() {
        assert!(Factor::try_new(-0.1).is_none());
        assert!(Factor::try_new(1.1).is_none());
        assert!(Factor::try_new(f64::NAN).is_none());
        assert_eq!(Factor::try_new(0.5).unwrap().get(), 0.5);
        assert_eq!(Factor::clamping(1.3).get(), 1.0);
        assert_eq!(Factor::clamping(-4.0).get(), 0.0);
        assert_eq!(Factor::clamping(f64::NAN).get(), 0.0);
    }
}
