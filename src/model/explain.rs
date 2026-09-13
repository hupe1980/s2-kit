//! Saying, in one value, what an instruction will actually do.
//!
//! A Resource Manager logs it so that an operator can see why the device did what it
//! did. A Customer Energy Manager calls it before sending, so that a plan built from
//! numbers is checked against the description it was built from. Both are asking the
//! same question — *what does this instruction mean, given what was described?* — and
//! both should get the same answer, which is the point of resolving it once here.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::{
    Limits, all_limits_at, blocking_timers, resolve_ddbc, resolve_frbc, resolve_ombc, resolve_ppbc,
    timer_label, transition_duration,
};
use crate::message::{Message, MessageKind};
use crate::types::common::{CommodityQuantity, PowerValue};
use crate::types::{Duration, Id, Timestamp};
use crate::validate::Context;

/// What an instruction means, read against the descriptions that were sent.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Explanation {
    /// The instruction's own identifier.
    pub instruction_id: Id,
    /// Which kind of instruction it is.
    pub kind: MessageKind,
    /// When it takes effect. In the past means as soon as possible.
    pub execution_time: Timestamp,
    /// Whether it is an abnormal-condition instruction.
    pub abnormal_condition: bool,
    /// The actuator it addresses, for the control types that have them.
    pub actuator: Option<Id>,
    /// The operation mode it selects, with its diagnostic label where there is one.
    pub operation_mode: Option<(Id, Option<String>)>,
    /// The factor it asks for.
    pub factor: Option<f64>,
    /// The power that factor implies.
    pub power: Vec<PowerValue>,
    /// FRBC: how fast the store will fill, in fill-level units per second.
    pub fill_rate: Option<f64>,
    /// DDBC: the supply rate the mode will deliver.
    pub supply_rate: Option<f64>,
    /// PEBC: the bounds in force when the envelope starts.
    pub limits: Vec<(CommodityQuantity, Limits)>,
    /// PPBC: the profile, container and sequence it chose, and how long it takes.
    pub sequence: Option<(Id, Id, Id, Duration)>,
    /// How long the move takes to take effect.
    pub transition_duration: Duration,
    /// Timers that have not finished and block the move this instruction asks for.
    pub blocked_by: Vec<(Id, Option<String>)>,
    /// Whether it uses something marked `abnormal_condition_only` without saying so.
    pub abnormal_only_misuse: bool,
    /// Why the instruction could not be read against the description, if it could not.
    ///
    /// An instruction naming an actuator nobody described, an operation mode that
    /// actuator does not have, or a factor outside `[0, 1]` has no power, no fill rate
    /// and no transition — and an [`Explanation`] that merely left those empty would be
    /// indistinguishable from one for an instruction that genuinely asks for zero watts.
    /// [`is_actionable`](Self::is_actionable) is false whenever this is set.
    pub unresolved: Option<super::ResolveError>,
    /// Anything else worth saying in a log line.
    pub notes: Vec<String>,
}

impl Explanation {
    /// Whether this instruction can be carried out right now, as far as the description
    /// can tell.
    #[must_use]
    pub fn is_actionable(&self) -> bool {
        self.unresolved.is_none() && self.blocked_by.is_empty() && !self.abnormal_only_misuse
    }

    /// The electrical power the instruction implies, if it names one.
    #[must_use]
    pub fn electric_power(&self) -> Option<f64> {
        self.power
            .iter()
            .find(|v| {
                v.commodity_quantity.commodity() == crate::types::common::Commodity::Electricity
            })
            .map(|v| v.value)
    }
}

impl core::fmt::Display for Explanation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} {}", self.kind, self.instruction_id)?;
        if let Some(actuator) = &self.actuator {
            write!(f, " on {actuator}")?;
        }
        if let Some((mode, label)) = &self.operation_mode {
            match label {
                Some(label) => write!(f, " → {label} ({mode})")?,
                None => write!(f, " → {mode}")?,
            }
        }
        if let Some(factor) = self.factor {
            write!(f, " at factor {factor}")?;
        }
        for value in &self.power {
            // The unit from the quantity, not a hard-coded `W`: four of the ten
            // quantities are litres, grams or degrees, and one of them is not a power.
            write!(
                f,
                ", {} {} {}",
                value.commodity_quantity.as_str(),
                value.value,
                value.commodity_quantity.unit()
            )?;
        }
        if let Some(rate) = self.fill_rate {
            write!(f, ", filling at {rate}/s")?;
        }
        if let Some(rate) = self.supply_rate {
            write!(f, ", supplying {rate}")?;
        }
        for (quantity, limits) in &self.limits {
            write!(
                f,
                ", {} bounded to {}..{} {}",
                quantity.as_str(),
                limits.lower,
                limits.upper,
                quantity.unit()
            )?;
        }
        if let Some((_, _, sequence, duration)) = &self.sequence {
            write!(f, ", running {sequence} for {duration}")?;
        }
        if !self.transition_duration.is_zero() {
            write!(f, ", taking effect after {}", self.transition_duration)?;
        }
        for (timer, label) in &self.blocked_by {
            match label {
                Some(label) => write!(f, "; blocked by {label} ({timer})")?,
                None => write!(f, "; blocked by {timer}")?,
            }
        }
        if let Some(why) = &self.unresolved {
            write!(f, "; unresolved: {why}")?;
        }
        if self.abnormal_only_misuse {
            write!(
                f,
                "; uses an abnormal-condition-only option without saying so"
            )?;
        }
        for note in &self.notes {
            write!(f, "; {note}")?;
        }
        Ok(())
    }
}

/// Explain an instruction against the descriptions a session has seen.
///
/// `None` for any message that is not an instruction.
#[must_use]
#[allow(clippy::too_many_lines)] // one arm per control type; splitting it would hide the symmetry
pub fn explain(message: &Message, ctx: &Context<'_>) -> Option<Explanation> {
    let instruction_id = message.instruction_id()?;
    let mut e = Explanation {
        instruction_id,
        kind: message.kind(),
        execution_time: Timestamp::UNIX_EPOCH,
        abnormal_condition: false,
        actuator: None,
        operation_mode: None,
        factor: None,
        power: Vec::new(),
        fill_rate: None,
        supply_rate: None,
        limits: Vec::new(),
        sequence: None,
        transition_duration: Duration::ZERO,
        blocked_by: Vec::new(),
        abnormal_only_misuse: false,
        unresolved: None,
        notes: Vec::new(),
    };

    match message {
        Message::FrbcInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            e.actuator = Some(i.actuator_id);
            e.factor = Some(i.operation_mode_factor);
            let Some(system) = ctx.frbc else {
                e.notes
                    .push("no FRBC.SystemDescription has been seen".into());
                e.unresolved = Some(super::ResolveError::NotDescribed);
                return Some(e);
            };
            match resolve_frbc(system, i, ctx.fill_level) {
                Ok(r) => {
                    e.operation_mode = Some((r.mode.id, r.mode.diagnostic_label.clone()));
                    e.power = r.power;
                    e.fill_rate = r.fill_rate;
                    e.abnormal_only_misuse = r.abnormal_only && !i.abnormal_condition;
                    if !system
                        .storage
                        .fill_level_range
                        .contains(ctx.fill_level.unwrap_or(f64::NAN))
                        && ctx.fill_level.is_some()
                    {
                        e.notes.push(
                            "the fill level is outside the storage's range, so the Resource \
                             Manager may ignore this"
                                .into(),
                        );
                    }
                    note_transition(
                        &mut e,
                        ctx,
                        &i.actuator_id,
                        &i.operation_mode,
                        &r.actuator.transitions,
                        &r.actuator.timers,
                    );
                }
                Err(err) => {
                    e.notes.push(err.to_string());
                    e.unresolved = Some(err);
                }
            }
        }
        Message::OmbcInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            e.factor = Some(i.operation_mode_factor);
            let Some(system) = ctx.ombc else {
                e.notes
                    .push("no OMBC.SystemDescription has been seen".into());
                e.unresolved = Some(super::ResolveError::NotDescribed);
                return Some(e);
            };
            match resolve_ombc(system, i) {
                Ok(r) => {
                    e.operation_mode = Some((r.mode.id, r.mode.diagnostic_label.clone()));
                    e.power = r.power;
                    e.abnormal_only_misuse = r.abnormal_only && !i.abnormal_condition;
                    note_transition(
                        &mut e,
                        ctx,
                        &Id::NIL,
                        &i.operation_mode_id,
                        &system.transitions,
                        &system.timers,
                    );
                }
                Err(err) => {
                    e.notes.push(err.to_string());
                    e.unresolved = Some(err);
                }
            }
        }
        Message::DdbcInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            e.actuator = Some(i.actuator_id);
            e.factor = Some(i.operation_mode_factor);
            let Some(system) = ctx.ddbc else {
                e.notes
                    .push("no DDBC.SystemDescription has been seen".into());
                e.unresolved = Some(super::ResolveError::NotDescribed);
                return Some(e);
            };
            match resolve_ddbc(system, i) {
                Ok(r) => {
                    e.operation_mode = Some((r.mode.id, r.mode.diagnostic_label.clone()));
                    e.power = r.power;
                    e.supply_rate = Some(r.supply_rate);
                    e.abnormal_only_misuse = r.abnormal_only && !i.abnormal_condition;
                    note_transition(
                        &mut e,
                        ctx,
                        &i.actuator_id,
                        &i.operation_mode_id,
                        &r.actuator.transitions,
                        &r.actuator.timers,
                    );
                }
                Err(err) => {
                    e.notes.push(err.to_string());
                    e.unresolved = Some(err);
                }
            }
        }
        Message::PebcInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            e.limits = all_limits_at(i, i.execution_time);
            if e.limits.is_empty() {
                e.notes
                    .push("the envelope bounds nothing at its own execution time".into());
            }
            if !ctx
                .pebc_constraints
                .iter()
                .any(|c| c.id == i.power_constraints_id)
                && !ctx.pebc_constraints.is_empty()
            {
                e.notes.push(format!(
                    "{} names no published PEBC.PowerConstraints",
                    i.power_constraints_id
                ));
            }
        }
        Message::PpbcScheduleInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            note_sequence(
                &mut e,
                ctx,
                i.power_profile_id,
                i.sequence_container_id,
                i.power_sequence_id,
            );
        }
        Message::PpbcStartInterruptionInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            note_sequence(
                &mut e,
                ctx,
                i.power_profile_id,
                i.sequence_container_id,
                i.power_sequence_id,
            );
            e.notes.push("pauses the sequence".into());
        }
        Message::PpbcEndInterruptionInstruction(i) => {
            e.execution_time = i.execution_time;
            e.abnormal_condition = i.abnormal_condition;
            note_sequence(
                &mut e,
                ctx,
                i.power_profile_id,
                i.sequence_container_id,
                i.power_sequence_id,
            );
            e.notes.push("resumes the sequence".into());
        }
        _ => return None,
    }

    if let Some(now) = ctx.now
        && e.execution_time <= now
    {
        e.notes
            .push("execution time has passed, so this means as soon as possible".into());
    }
    Some(e)
}

fn note_transition(
    e: &mut Explanation,
    ctx: &Context<'_>,
    actuator: &Id,
    to: &Id,
    transitions: &[crate::types::common::Transition],
    timers: &[crate::types::common::Timer],
) {
    let Some(active) = ctx
        .active_modes
        .iter()
        .find(|(a, _)| a == actuator)
        .map(|(_, m)| *m)
    else {
        return;
    };
    if active == *to {
        e.notes
            .push("already in this operation mode; only the factor changes".into());
        return;
    }
    e.transition_duration = transition_duration(transitions, &active, to);
    if let Some(now) = ctx.now {
        e.blocked_by = blocking_timers(transitions, &active, to, actuator, ctx.timers, now)
            .into_iter()
            .map(|id| (id, timer_label(timers, &id).map(ToString::to_string)))
            .collect();
    }
    if transitions
        .iter()
        .all(|t| !(t.from == active && t.to == *to))
    {
        e.notes
            .push(format!("no transition is described from {active}"));
    }
}

fn note_sequence(e: &mut Explanation, ctx: &Context<'_>, profile: Id, container: Id, sequence: Id) {
    match resolve_ppbc(ctx.ppbc_profiles, &profile, &container, &sequence) {
        Ok(r) => e.sequence = Some((profile, container, sequence, r.duration)),
        Err(err) => {
            e.sequence = Some((profile, container, sequence, Duration::ZERO));
            if !ctx.ppbc_profiles.is_empty() {
                e.notes.push(err.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_instruction_that_did_not_resolve_is_not_actionable() {
        use super::*;
        use crate::model::ResolveError;
        use crate::types::common::{Commodity, NumberRange, PowerRange};
        use crate::types::frbc;
        use alloc::vec;

        // `is_actionable` used to mean "nothing is blocking it", which is true of an
        // instruction whose actuator nobody described — it has no transitions, so nothing
        // can block it. An application that asked the question got `true` and an empty
        // `power`, which is indistinguishable from an instruction that genuinely asks for
        // zero watts.
        let system = frbc::SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: Timestamp::UNIX_EPOCH,
            actuators: vec![frbc::ActuatorDescription {
                id: Id::new_const("actuator1"),
                diagnostic_label: None,
                supported_commodities: vec![Commodity::Electricity],
                operation_modes: vec![frbc::OperationMode {
                    id: Id::new_const("om1"),
                    diagnostic_label: None,
                    elements: vec![frbc::OperationModeElement {
                        fill_level_range: NumberRange::new(0.0, 100.0),
                        fill_rate: NumberRange::new(0.0, 0.002),
                        power_ranges: vec![PowerRange::new(
                            0.0,
                            5000.0,
                            CommodityQuantity::ElectricPower3PhaseSymmetric,
                        )],
                        running_costs: None,
                    }],
                    abnormal_condition_only: false,
                }],
                transitions: vec![],
                timers: vec![],
            }],
            storage: frbc::StorageDescription {
                diagnostic_label: None,
                fill_level_label: None,
                provides_leakage_behaviour: false,
                provides_fill_level_target_profile: false,
                provides_usage_forecast: false,
                fill_level_range: NumberRange::new(0.0, 100.0),
            },
        };
        let ctx = Context {
            frbc: Some(&system),
            ..Context::empty()
        };
        let instruct = |actuator: &'static str, factor: f64| {
            Message::from(frbc::Instruction {
                message_id: Id::new_const("m2"),
                id: Id::new_const("i1"),
                actuator_id: Id::new_const(actuator),
                operation_mode: Id::new_const("om1"),
                operation_mode_factor: factor,
                execution_time: Timestamp::UNIX_EPOCH,
                abnormal_condition: false,
            })
        };

        // The happy case still is actionable.
        let good = explain(&instruct("actuator1", 0.5), &ctx).expect("an instruction");
        assert!(good.unresolved.is_none());
        assert!(good.is_actionable());
        assert!(!good.power.is_empty());

        // An actuator nobody described.
        let ghost = explain(&instruct("ghost", 0.5), &ctx).expect("an instruction");
        assert_eq!(
            ghost.unresolved,
            Some(ResolveError::UnknownActuator(Id::new_const("ghost")))
        );
        assert!(!ghost.is_actionable());
        assert!(ghost.power.is_empty());
        assert!(ghost.to_string().contains("unresolved"));

        // A factor the arithmetic cannot use.
        let bad = explain(&instruct("actuator1", 1.3), &ctx).expect("an instruction");
        assert_eq!(bad.unresolved, Some(ResolveError::BadFactor(1.3)));
        assert!(!bad.is_actionable());

        // And no description at all is its own answer, not a silently empty one.
        let blind = explain(&instruct("actuator1", 0.5), &Context::empty()).expect("one");
        assert_eq!(blind.unresolved, Some(ResolveError::NotDescribed));
        assert!(!blind.is_actionable());
    }

    use super::*;
    use crate::types::common::{Commodity, NumberRange, PowerRange, Transition};
    use crate::types::frbc;
    use alloc::vec;

    fn system() -> frbc::SystemDescription {
        frbc::SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: Timestamp::UNIX_EPOCH,
            actuators: vec![frbc::ActuatorDescription {
                id: Id::new_const("actuator1"),
                diagnostic_label: Some("EV charger".into()),
                supported_commodities: vec![Commodity::Electricity],
                operation_modes: vec![
                    frbc::OperationMode {
                        id: Id::new_const("om1"),
                        diagnostic_label: Some("Off".into()),
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
                    frbc::OperationMode {
                        id: Id::new_const("om2"),
                        diagnostic_label: Some("Charging".into()),
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
                    },
                ],
                transitions: vec![Transition {
                    id: Id::new_const("transition1"),
                    from: Id::new_const("om1"),
                    to: Id::new_const("om2"),
                    start_timers: vec![],
                    blocking_timers: vec![Id::new_const("timer0")],
                    transition_costs: None,
                    transition_duration: Some(Duration::from_secs(3)),
                    abnormal_condition_only: false,
                }],
                timers: vec![crate::types::common::Timer {
                    id: Id::new_const("timer0"),
                    diagnostic_label: Some("Minimum off time".into()),
                    duration: Duration::from_secs(3600),
                }],
            }],
            storage: frbc::StorageDescription {
                diagnostic_label: None,
                fill_level_label: None,
                provides_leakage_behaviour: false,
                provides_fill_level_target_profile: false,
                provides_usage_forecast: false,
                fill_level_range: NumberRange::new(0.0, 100.0),
            },
        }
    }

    fn instruction() -> Message {
        Message::from(frbc::Instruction {
            message_id: Id::new_const("m2"),
            id: Id::new_const("instr0"),
            actuator_id: Id::new_const("actuator1"),
            operation_mode: Id::new_const("om2"),
            operation_mode_factor: 0.5,
            execution_time: "2024-01-01T12:00:00Z".parse().unwrap(),
            abnormal_condition: false,
        })
    }

    #[test]
    fn an_explanation_names_the_power_and_the_mode_a_human_would_recognise() {
        let system = system();
        let ctx = Context {
            frbc: Some(&system),
            fill_level: Some(50.0),
            ..Context::empty()
        };
        let e = explain(&instruction(), &ctx).unwrap();
        assert_eq!(e.operation_mode.clone().unwrap().1.unwrap(), "Charging");
        assert!((e.power[0].value - 6200.0).abs() < 1e-9);
        assert!(e.is_actionable());
        let rendered = e.to_string();
        assert!(rendered.contains("Charging"), "{rendered}");
        assert!(rendered.contains("6200"), "{rendered}");
    }

    #[test]
    fn an_explanation_says_when_a_timer_is_in_the_way() {
        let system = system();
        let now: Timestamp = "2024-01-01T12:00:00Z".parse().unwrap();
        let active = [(Id::new_const("actuator1"), Id::new_const("om1"))];
        let timers = [crate::validate::TimerState {
            actuator: Id::new_const("actuator1"),
            timer: Id::new_const("timer0"),
            finished_at: "2024-01-01T12:30:00Z".parse().unwrap(),
        }];
        let ctx = Context {
            frbc: Some(&system),
            fill_level: Some(50.0),
            active_modes: &active,
            timers: &timers,
            now: Some(now),
            ..Context::empty()
        };
        let e = explain(&instruction(), &ctx).unwrap();
        assert!(!e.is_actionable());
        assert_eq!(e.blocked_by.len(), 1);
        assert_eq!(e.blocked_by[0].1.as_deref(), Some("Minimum off time"));
        assert_eq!(e.transition_duration, Duration::from_secs(3));
        assert!(e.to_string().contains("Minimum off time"));
    }

    #[test]
    fn an_explanation_without_a_description_says_so_rather_than_guessing() {
        let e = explain(&instruction(), &Context::empty()).unwrap();
        assert!(e.power.is_empty());
        assert_eq!(e.notes, vec!["no FRBC.SystemDescription has been seen"]);
    }

    #[test]
    fn nothing_to_explain_about_a_measurement() {
        let m = Message::from(frbc::StorageStatus {
            message_id: Id::new_const("m1"),
            present_fill_level: 50.0,
        });
        assert!(explain(&m, &Context::empty()).is_none());
    }
}
