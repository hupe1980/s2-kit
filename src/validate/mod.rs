//! The semantic validator: everything JSON Schema cannot say.
//!
//! A message can satisfy the schema completely and still be wrong. An operation mode
//! whose fill-level bands leave a gap, an envelope outside the bounds the Resource
//! Manager published, an instruction naming an actuator that does not exist, a
//! percentile band below the one beneath it — all of these are schema-valid, and all of
//! them are what `INVALID_CONTENT` exists for: "Message contents is invalid (e.g.
//! contains a non-existing ID). Somewhat equivalent to BAD_REQUEST in HTTP."
//!
//! # Three layers, never collapsed
//!
//! | Layer | Answered by | A failure becomes |
//! |---|---|---|
//! | Is it JSON, and does it match the schema? | [`crate::codec`] | `INVALID_DATA` / `INVALID_MESSAGE` |
//! | Is it internally consistent? | this module, with [`Context::empty`] | `INVALID_CONTENT` |
//! | Is it consistent with the session? | this module, with a full [`Context`] | `INVALID_CONTENT` |
//!
//! The types accept whatever the schema accepts — a factor of 1.3 is representable —
//! because a proxy has to be able to carry a message it would refuse to send, and
//! because answering `INVALID_CONTENT` requires having decoded the message first.
//!
//! # Severity
//!
//! An [`Error`](Severity::Error) is a requirement the standard states plainly. A
//! [`Warning`](Severity::Warning) is one it leaves ambiguous — and there are more of
//! those than anyone would like, each citing its `[s2-json #n]` issue or its `E` number
//! as errata. Warnings never refuse traffic: another conforming
//! implementation is entitled to read the same silence differently.
//!
//! ```
//! use s2_kit::prelude::*;
//! use s2_kit::validate::{Context, Validate, rules};
//!
//! let message = decode(
//!     r#"{"message_type":"FRBC.Instruction","message_id":"m1","id":"i1",
//!         "actuator_id":"a1","operation_mode":"om1","operation_mode_factor":1.3,
//!         "execution_time":"2019-08-24T14:15:22Z","abnormal_condition":false}"#,
//! )?;
//!
//! let report = message.validate(&Context::empty());
//! assert!(report.has_errors());
//! assert_eq!(report.first_error().unwrap().rule, rules::FACTOR_RANGE);
//! // The rule identifier is what travels in the reception status we send back.
//! assert!(report.diagnostic_label().unwrap().starts_with("S2-NUM-004"));
//! # Ok::<(), s2_kit::DecodeError>(())
//! ```

pub mod rules;
pub mod state;

mod messages;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::message::MessageKind;
use crate::schema;
use crate::types::common::{
    CommodityQuantity, ControlType, EnergyManagementRole, InstructionStatus, NumberRange,
    ReceptionStatusValues, ResourceManagerDetails, RevokableObjects,
};
use crate::types::{Duration, Id, Timestamp, WireProfile, ddbc, frbc, ombc, pebc, ppbc};

pub use rules::Rule;
pub use state::{Allowance, Phase, allowed};

/// A stable rule identifier, such as `S2-FRBC-004`.
///
/// Identifiers never change meaning. They are printed in the
/// `diagnostic_label` of every failing reception status this crate sends, and listed in
/// the published [rule catalogue](https://hupe1980.github.io/s2-kit/reference/rules/).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId(pub &'static str);

impl RuleId {
    /// The identifier as a string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The catalogue entry, if this is a known rule.
    #[must_use]
    pub fn rule(self) -> Option<&'static Rule> {
        Rule::find(self)
    }
}

impl core::fmt::Display for RuleId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.0)
    }
}

/// Whether breaking a rule makes a message invalid, or merely suspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The standard states the requirement plainly.
    Error,
    /// The standard is ambiguous, silent or self-contradictory here.
    Warning,
}

/// One thing that is wrong with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Which rule was broken.
    pub rule: RuleId,
    /// How seriously.
    pub severity: Severity,
    /// A JSON pointer to the offending value, `""` for the message as a whole.
    pub path: String,
    /// What is wrong, in a sentence.
    pub message: String,
    /// The identifiers involved, so a tool can link them.
    pub related: Vec<Id>,
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.rule, self.message)
        } else {
            write!(f, "{} at {}: {}", self.rule, self.path, self.message)
        }
    }
}

/// Everything wrong with one message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    violations: Vec<Violation>,
}

impl Report {
    /// An empty report.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            violations: Vec::new(),
        }
    }

    /// Record a violation. The severity comes from the catalogue.
    pub fn push(&mut self, rule: RuleId, path: &str, message: impl Into<String>) {
        let severity = rule.rule().map_or(Severity::Error, |r| r.severity);
        self.violations.push(Violation {
            rule,
            severity,
            path: path.to_string(),
            message: message.into(),
            related: Vec::new(),
        });
    }

    /// Record a violation that involves particular identifiers.
    pub fn push_related(
        &mut self,
        rule: RuleId,
        path: &str,
        message: impl Into<String>,
        related: impl IntoIterator<Item = Id>,
    ) {
        self.push(rule, path, message);
        if let Some(last) = self.violations.last_mut() {
            last.related = related.into_iter().collect();
        }
    }

    /// Every violation, errors and warnings alike.
    #[must_use]
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// Only the errors.
    pub fn errors(&self) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Error)
    }

    /// Only the warnings.
    pub fn warnings(&self) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
    }

    /// Whether anything at all was reported.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    /// Whether the message should be refused.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// The first error, which is the one worth telling the peer about.
    #[must_use]
    pub fn first_error(&self) -> Option<&Violation> {
        self.errors().next()
    }

    /// Whether a particular rule fired.
    #[must_use]
    pub fn contains(&self, rule: RuleId) -> bool {
        self.violations.iter().any(|v| v.rule == rule)
    }

    /// Every rule that fired, in order.
    pub fn fired(&self) -> impl Iterator<Item = RuleId> + '_ {
        self.violations.iter().map(|v| v.rule)
    }

    /// The reception status a receiver should answer with.
    #[must_use]
    pub fn reception_status(&self) -> ReceptionStatusValues {
        if self.has_errors() {
            ReceptionStatusValues::InvalidContent
        } else {
            ReceptionStatusValues::Ok
        }
    }

    /// The `diagnostic_label` for that reception status: the first error, rule
    /// identifier first, so that errors are greppable across a fleet.
    #[must_use]
    pub fn diagnostic_label(&self) -> Option<String> {
        self.first_error().map(ToString::to_string)
    }

    /// Fold another report into this one.
    pub fn extend(&mut self, other: Report) {
        self.violations.extend(other.violations);
    }
}

impl core::fmt::Display for Report {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for (i, v) in self.violations.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{v}")?;
        }
        Ok(())
    }
}

/// When one timer of one actuator finishes.
///
/// A three-field struct rather than a `(Id, Timestamp)` pair because timer identifiers
/// are **not** unique on their own. `S2J schemas/Timer.id`: "Must be unique in the scope
/// of the OMBC.SystemDescription, FRBC.ActuatorDescription or DDBC.ActuatorDescription in
/// which it is used" — so an FRBC system with two actuators may legally call a timer
/// `timer1` on each, and a table keyed by the identifier alone would let one actuator's
/// running timer block the other's transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerState {
    /// The actuator whose timer this is.
    ///
    /// [`Id::NIL`] for OMBC, whose timers belong to the `OMBC.SystemDescription` itself
    /// rather than to an actuator — the same stand-in
    /// [`Context::active_modes`] uses for OMBC's single state machine.
    pub actuator: Id,
    /// The timer.
    pub timer: Id,
    /// When it finishes. In the past means it has finished; "if the timer was never
    /// started, the value can be an arbitrary DateTimeStamp in the past", so a timer
    /// nobody has reported on is not the same as a finished one.
    pub finished_at: Timestamp,
}

/// What the cross-message rules need to know.
///
/// [`Context::empty`] runs the rules that need nothing but the message in front of them,
/// which is what makes the validator usable on a lone JSON file. A session fills in the
/// rest.
/// Fields are public and the struct is deliberately **not** `#[non_exhaustive]`: a
/// proxy, an analyzer or a test builds one with `Context { frbc: Some(&system),
/// ..Context::empty() }`, and a marker that forbids struct-update syntax would take that
/// away for the sake of a guarantee this crate does not need before 1.0.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// The negotiated wire profile.
    pub profile: WireProfile,
    /// Which role sent the message being validated.
    pub sender: Option<EnergyManagementRole>,
    /// Where the session has got to: negotiating, connected, or a control type active.
    ///
    /// The row of `S2C §State of communication` this message is being judged against.
    pub phase: Phase,
    /// Whether the session is running under S2 Connect, where the handshake messages
    /// must not appear.
    pub s2_connect: bool,
    /// The resource's details, once it has sent them.
    pub details: Option<&'a ResourceManagerDetails>,
    /// The effective FRBC system description.
    pub frbc: Option<&'a frbc::SystemDescription>,
    /// The effective OMBC system description.
    pub ombc: Option<&'a ombc::SystemDescription>,
    /// The effective DDBC system description.
    pub ddbc: Option<&'a ddbc::SystemDescription>,
    /// Power constraints currently published.
    pub pebc_constraints: &'a [pebc::PowerConstraints],
    /// Power profiles currently published.
    pub ppbc_profiles: &'a [ppbc::PowerProfileDefinition],
    /// Instruction identifiers the CEM has already used.
    pub instructions: &'a [Id],
    /// `(message_id, instruction_id)` for each of them, so that a status update naming
    /// the wrong one of the two can say which mistake was made.
    pub instruction_messages: &'a [(Id, Id)],
    /// The latest status of each instruction.
    pub instruction_statuses: &'a [(Id, InstructionStatus)],
    /// Message identifiers already seen this session.
    pub seen_message_ids: &'a [Id],
    /// Objects published this session, and so revokable.
    pub published: &'a [(RevokableObjects, Id)],
    /// The operation mode each actuator is in, keyed by actuator (`Id::NIL` for OMBC,
    /// which has a single machine).
    pub active_modes: &'a [(Id, Id)],
    /// When each actuator's timers finish.
    pub timers: &'a [TimerState],
    /// The present fill level, once the storage has reported one.
    pub fill_level: Option<f64>,
    /// The local clock, for the skew rule.
    pub now: Option<Timestamp>,
    /// How far a peer's clock may differ before it is worth reporting.
    pub skew_tolerance: Duration,
}

impl<'a> Context<'a> {
    /// A context that knows nothing, so only the intra-message rules run.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            skew_tolerance: Duration::from_secs(30),
            ..Self::default()
        }
    }

    /// With the resource's details, which several rules need.
    #[must_use]
    pub fn with_details(mut self, details: &'a ResourceManagerDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// With a clock, which the timer and skew rules need.
    #[must_use]
    pub fn at(mut self, now: Timestamp) -> Self {
        self.now = Some(now);
        self
    }

    /// With the role that sent the message, which the state rules need.
    #[must_use]
    pub fn sent_by(mut self, sender: EnergyManagementRole) -> Self {
        self.sender = Some(sender);
        self
    }

    /// With a control type active, which the state rules need.
    #[must_use]
    pub fn active(mut self, control_type: ControlType) -> Self {
        self.phase = Phase::selected(control_type);
        self
    }

    /// With the session in a particular phase, which the state rules need.
    #[must_use]
    pub const fn in_phase(mut self, phase: Phase) -> Self {
        self.phase = phase;
        self
    }

    /// The control type that is active, if one that can be instructed is.
    #[must_use]
    pub const fn active_control_type(&self) -> Option<ControlType> {
        self.phase.active_control_type()
    }

    /// Whether cross-message rules can run at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sender.is_none() && self.details.is_none() && self.phase == Phase::Connected
    }

    fn timer_finished_at(&self, actuator: &Id, timer: &Id) -> Option<Timestamp> {
        self.timers
            .iter()
            .find(|t| t.actuator == *actuator && t.timer == *timer)
            .map(|t| t.finished_at)
    }

    fn active_mode_of(&self, actuator: &Id) -> Option<Id> {
        self.active_modes
            .iter()
            .find(|(a, _)| a == actuator)
            .map(|(_, m)| *m)
    }
}

/// Something that can be checked against the standard's semantics.
pub trait Validate {
    /// Check, recording everything wrong at `path` and below.
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report);

    /// Check, returning a fresh report.
    fn validate(&self, ctx: &Context<'_>) -> Report {
        let mut report = Report::new();
        self.validate_at("", ctx, &mut report);
        report
    }
}

/// The rule catalogue as Markdown, for the documentation site and `s2-kit rules`.
///
/// Generated rather than written, so the document and the code cannot disagree about
/// which rules exist or how severe they are.
#[must_use]
#[allow(clippy::format_push_string)] // a few dozen rows; clarity beats the allocation
pub fn rules_markdown() -> String {
    let mut out = String::from(
        "# Rule catalogue\n\n\
         Generated from `s2_kit::validate::rules::RULES` — do not edit.\n\n\
         Every rule quotes the sentence it implements. An **error** is a requirement the\n\
         standard states plainly; a **warning** is one it leaves ambiguous, silent or\n\
         self-contradictory, and never refuses traffic. The identifier travels in the\n\
         `diagnostic_label` of every failing `ReceptionStatus` this crate sends.\n\n",
    );
    let errors = rules::RULES
        .iter()
        .filter(|r| r.severity == Severity::Error)
        .count();
    out.push_str(&format!(
        "{} rules: {} errors, {} warnings.\n\n",
        rules::RULES.len(),
        errors,
        rules::RULES.len() - errors
    ));
    out.push_str("| Rule | Severity | Needs session | Checks | Source |\n|---|---|---|---|---|\n");
    for rule in rules::RULES {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            rule.id,
            match rule.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            },
            if rule.needs_context { "yes" } else { "no" },
            rule.summary,
            rule.source.replace('|', "\\|")
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Shared checks
// ---------------------------------------------------------------------------

pub(crate) fn child(path: &str, name: &str) -> String {
    format!("{path}/{name}")
}

pub(crate) fn index(path: &str, i: usize) -> String {
    format!("{path}/{i}")
}

/// A number that cannot be encoded is not a number S2 can carry.
pub(crate) fn check_finite(value: f64, path: &str, out: &mut Report) {
    if !value.is_finite() {
        out.push(
            rules::NON_FINITE,
            path,
            format!("{value} cannot be represented in JSON"),
        );
    }
}

/// A range whose ends are a *factor-0 value* and a *factor-1 value*, not a low and a
/// high.
///
/// `S2J schemas/FRBC.OperationModeElement.fill_rate`: "The lower_boundary of the
/// NumberRange is associated with an operation_mode_factor of 0, the upper_boundary is
/// associated with an operation_mode_factor of 1." Nothing says the first is the
/// smaller, and for a discharging battery it is not: its fill rate runs from 0 down to
/// −0.0025, and its power from 0 down to −5000 W. Only finiteness is checked here.
pub(crate) fn check_interpolated(range: NumberRange, path: &str, out: &mut Report) {
    check_finite(range.start_of_range, &child(path, "start_of_range"), out);
    check_finite(range.end_of_range, &child(path, "end_of_range"), out);
}

/// A range whose start must not be after its end, where the standard says so.
pub(crate) fn check_range(range: NumberRange, path: &str, out: &mut Report) {
    check_finite(range.start_of_range, &child(path, "start_of_range"), out);
    check_finite(range.end_of_range, &child(path, "end_of_range"), out);
    if range.is_finite() && range.start_of_range > range.end_of_range {
        out.push(
            rules::RANGE_ORDER,
            path,
            format!(
                "start_of_range {} is after end_of_range {}",
                range.start_of_range, range.end_of_range
            ),
        );
    }
}

/// A band whose start must be *strictly* below its end, as fill-level bands are.
pub(crate) fn check_band(range: NumberRange, path: &str, out: &mut Report) {
    check_finite(range.start_of_range, &child(path, "start_of_range"), out);
    check_finite(range.end_of_range, &child(path, "end_of_range"), out);
    if range.is_finite() && range.start_of_range >= range.end_of_range {
        out.push(
            rules::RANGE_STRICT_ORDER,
            path,
            format!(
                "fill level band {}..{} is empty; the start must be below the end",
                range.start_of_range, range.end_of_range
            ),
        );
    }
}

pub(crate) fn check_factor(factor: f64, path: &str, out: &mut Report) {
    check_finite(factor, path, out);
    if factor.is_finite() && !(0.0..=1.0).contains(&factor) {
        out.push(
            rules::FACTOR_RANGE,
            path,
            format!("operation mode factor {factor} is outside [0, 1]"),
        );
    }
}

/// Checks an array against the `minItems`/`maxItems` the schema gives it.
pub(crate) fn check_array(
    type_name: &str,
    property: &str,
    len: usize,
    path: &str,
    out: &mut Report,
) {
    let Some(bounds) = schema::type_spec(type_name)
        .and_then(|t| t.property(property))
        .and_then(|p| p.kind.bounds())
    else {
        return;
    };
    if !bounds.accepts(len as u64) {
        out.push(
            rules::ARRAY_BOUNDS,
            &child(path, property),
            format!(
                "{len} items, but the schema allows {}..={}",
                bounds.min.unwrap_or(0),
                bounds
                    .max
                    .map_or_else(|| "unbounded".to_string(), |m| m.to_string())
            ),
        );
    }
}

/// "at most one item per commodity_quantity", which appears on every power array.
pub(crate) fn check_unique_quantities<I>(quantities: I, path: &str, out: &mut Report)
where
    I: IntoIterator<Item = CommodityQuantity>,
{
    let mut seen: Vec<CommodityQuantity> = Vec::new();
    let mut per_phase = false;
    let mut symmetric = false;
    for q in quantities {
        if seen.contains(&q) {
            out.push(
                rules::ONE_VALUE_PER_QUANTITY,
                path,
                format!("{} appears more than once", q.as_str()),
            );
        } else {
            seen.push(q);
        }
        per_phase |= q.is_per_phase_electric();
        symmetric |= q == CommodityQuantity::ElectricPower3PhaseSymmetric;
    }
    if per_phase && symmetric {
        out.push(
            rules::PHASE_EXCLUSIVITY,
            path,
            "per-phase and three-phase-symmetric electric power are both present; \
             a CEM cannot tell whether to add them or choose between them",
        );
    }
}

/// Identifiers must be unique within the scope the standard names.
pub(crate) fn check_unique_ids<'i, I>(ids: I, scope: &str, path: &str, out: &mut Report)
where
    I: IntoIterator<Item = &'i Id>,
{
    let mut seen: Vec<&Id> = Vec::new();
    for id in ids {
        if seen.contains(&id) {
            out.push_related(
                rules::DUPLICATE_ID,
                path,
                format!("{id} appears more than once within {scope}"),
                [*id],
            );
        } else {
            seen.push(id);
        }
    }
}

/// Bands must tile their range with no gap and no overlap.
pub(crate) fn check_contiguous(
    ranges: &[NumberRange],
    rule: RuleId,
    what: &str,
    path: &str,
    out: &mut Report,
) {
    for (i, pair) in ranges.windows(2).enumerate() {
        let [a, b] = pair else { continue };
        if !a.is_finite() || !b.is_finite() {
            continue;
        }
        // A few ULPs rather than one: a boundary a Resource Manager *computed* — 100/3,
        // a kWh converted from Wh — lands a bit or two either side of the same boundary
        // written out by hand, and a rule that calls that a gap reports arithmetic rather
        // than a defect. A real gap in a fill-level band is a whole unit, not a bit.
        let tolerance =
            4.0 * f64::EPSILON * a.end_of_range.abs().max(b.start_of_range.abs()).max(1.0);
        if (a.end_of_range - b.start_of_range).abs() > tolerance {
            out.push(
                rule,
                &index(path, i + 1),
                format!(
                    "{what} are not contiguous: the previous band ends at {} and this one starts at {}",
                    a.end_of_range, b.start_of_range
                ),
            );
        }
    }
}

/// Confidence bands run from the lower limit up to the upper.
pub(crate) fn check_percentiles(
    bands: &[(&'static str, Option<f64>); 7],
    path: &str,
    out: &mut Report,
) {
    let mut previous: Option<(&str, f64)> = None;
    for (name, value) in bands {
        let Some(value) = *value else { continue };
        check_finite(value, &child(path, name), out);
        if let Some((previous_name, previous_value)) = previous
            && value.is_finite()
            && previous_value.is_finite()
            && value < previous_value
        {
            out.push(
                rules::PERCENTILE_ORDER,
                &child(path, name),
                format!("{name} ({value}) is below {previous_name} ({previous_value})"),
            );
        }
        if value.is_finite() {
            previous = Some((name, value));
        }
    }
}

/// How far ahead a *scheduled* instant may be before it reads as a mistake.
///
/// A year. `valid_from`, `start_time` and `execution_time` are allowed to be in the
/// future — that is what they are for — so the only thing worth saying about one is that
/// it is so far away that nothing will still be running when it arrives.
pub(crate) const SCHEDULING_HORIZON: Duration = Duration::from_secs(365 * 86_400);

/// A timestamp that says when something *happened*, checked against the local clock.
///
/// `measurement_timestamp`, `transition_timestamp` and an `InstructionStatusUpdate`'s
/// `timestamp` all describe the past. One of them in the future means the two clocks
/// disagree, and by how much is exactly what
/// [`skew_tolerance`](Context::skew_tolerance) is for — the knob would be decoration if
/// this were the fixed horizon the scheduled instants need.
///
/// Nothing is ever reported for a timestamp in the *past*: the standard says plainly that
/// one means "already" (`S2J messages/FRBC.TimerStatus.finished_at`), and a device whose
/// clock is a minute slow is a device, not a fault.
pub(crate) fn check_observed(at: Timestamp, path: &str, ctx: &Context<'_>, out: &mut Report) {
    let Some(now) = ctx.now else { return };
    let ahead = at.saturating_duration_since(now);
    if ahead > ctx.skew_tolerance {
        out.push(
            rules::TIMESTAMP_SKEW,
            path,
            format!(
                "{at} says something has already happened, but it is {} ms ahead of \
                 the local clock ({now}), which tolerates {} ms of skew",
                ahead.as_millis(),
                ctx.skew_tolerance.as_millis()
            ),
        );
    }
}

/// A timestamp that says when something *will* happen, checked for plausibility only.
pub(crate) fn check_scheduled(at: Timestamp, path: &str, ctx: &Context<'_>, out: &mut Report) {
    let Some(now) = ctx.now else { return };
    let ahead = at.saturating_duration_since(now);
    if ahead > SCHEDULING_HORIZON {
        out.push(
            rules::TIMESTAMP_SKEW,
            path,
            format!("{at} is more than a year ahead of the local clock ({now})"),
        );
    }
}

/// Transitions may only name modes and timers of their own scope.
pub(crate) fn check_transitions(
    transitions: &[crate::types::common::Transition],
    mode_ids: &[Id],
    timer_ids: &[Id],
    rule: RuleId,
    path: &str,
    out: &mut Report,
) {
    for (i, transition) in transitions.iter().enumerate() {
        let at = index(path, i);
        for (field, id) in [("from", &transition.from), ("to", &transition.to)] {
            if !mode_ids.contains(id) {
                out.push_related(
                    rule,
                    &child(&at, field),
                    format!("{id} is not an operation mode of this description"),
                    [*id],
                );
            }
        }
        for (field, ids) in [
            ("start_timers", &transition.start_timers),
            ("blocking_timers", &transition.blocking_timers),
        ] {
            for id in ids {
                if !timer_ids.contains(id) {
                    out.push_related(
                        rule,
                        &child(&at, field),
                        format!("{id} is not a timer of this description"),
                        [*id],
                    );
                }
            }
        }
        if let Some(cost) = transition.transition_costs {
            check_finite(cost, &child(&at, "transition_costs"), out);
        }
    }
}

/// The shared part of every instruction check: does the mode exist, is it allowed now,
/// and is the move it asks for blocked?
pub(crate) struct ModeCheck<'a> {
    pub actuator: Option<&'a Id>,
    pub mode: &'a Id,
    pub abnormal_condition: bool,
    pub abnormal_only: bool,
    pub transitions: &'a [crate::types::common::Transition],
    pub timers: &'a [crate::types::common::Timer],
}

pub(crate) fn check_instruction_context(
    check: &ModeCheck<'_>,
    path: &str,
    ctx: &Context<'_>,
    out: &mut Report,
) {
    if check.abnormal_only && !check.abnormal_condition {
        out.push_related(
            rules::ABNORMAL_ONLY,
            path,
            format!(
                "{} may only be used during an abnormal condition, \
                 and abnormal_condition is false",
                check.mode
            ),
            [*check.mode],
        );
    }

    let actuator = check.actuator.copied().unwrap_or(Id::NIL);
    let Some(active) = ctx.active_mode_of(&actuator) else {
        return;
    };
    if active == *check.mode {
        // the S2 documentation, *Operation modes*: "A CEM is also always allowed to request the
        // RM to go the same Operation Mode as the currently active one, but with a
        // different factor. It is not necessary for the RM to define a Transition from
        // that Operation Mode to itself."
        return;
    }

    let Some(transition) = check
        .transitions
        .iter()
        .find(|t| t.from == active && t.to == *check.mode)
    else {
        out.push_related(
            rules::NO_SUCH_TRANSITION,
            path,
            format!("no transition is described from {active} to {}", check.mode),
            [active, *check.mode],
        );
        return;
    };

    if transition.abnormal_condition_only && !check.abnormal_condition {
        out.push_related(
            rules::ABNORMAL_ONLY,
            path,
            format!(
                "transition {} may only be used during an abnormal condition",
                transition.id
            ),
            [transition.id],
        );
    }

    if let Some(now) = ctx.now {
        for timer_id in &transition.blocking_timers {
            // A timer nobody has reported on is not a finished timer: the standard says
            // an unstarted timer carries "an arbitrary DateTimeStamp in the past", which
            // means silence tells us nothing either way.
            if let Some(finished_at) = ctx.timer_finished_at(&actuator, timer_id)
                && finished_at > now
            {
                let label = check
                    .timers
                    .iter()
                    .find(|t| t.id == *timer_id)
                    .and_then(|t| t.diagnostic_label.as_deref())
                    .unwrap_or("timer");
                out.push_related(
                    rules::BLOCKED_BY_TIMER,
                    path,
                    format!("{label} {timer_id} does not finish until {finished_at}"),
                    [*timer_id],
                );
            }
        }
    }
}

/// Rules that apply to every message, whatever it is.
pub(crate) fn check_envelope(
    kind: MessageKind,
    message_id: Option<Id>,
    ctx: &Context<'_>,
    out: &mut Report,
) {
    if let Some(sender) = ctx.sender {
        match state::allowed(ctx.phase, sender, kind) {
            Allowance::Yes => {}
            Allowance::WrongState => out.push(
                rules::NOT_ALLOWED_IN_STATE,
                "",
                format!(
                    "{kind} is not allowed while the session is {}",
                    ctx.phase.label()
                ),
            ),
            Allowance::WrongRole => out.push(
                rules::NOT_ALLOWED_FOR_ROLE,
                "",
                format!("{kind} is not sent by a {sender:?}"),
            ),
        }
    }

    if ctx.s2_connect
        && matches!(
            kind,
            MessageKind::Handshake | MessageKind::HandshakeResponse
        )
    {
        out.push(
            rules::HANDSHAKE_UNDER_CONNECT,
            "",
            "under S2 Connect the version is negotiated during session initiation, \
             and handshake messages are redundant",
        );
    }

    if let Some(id) = message_id
        && ctx.seen_message_ids.contains(&id)
    {
        out.push_related(
            rules::DUPLICATE_MESSAGE_ID,
            "/message_id",
            format!("{id} has already been used on this session"),
            [id],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_ok_until_an_error_is_recorded() {
        let mut report = Report::new();
        assert!(report.is_empty());
        assert_eq!(report.reception_status(), ReceptionStatusValues::Ok);

        report.push(rules::PHASE_EXCLUSIVITY, "/values", "mixed phases");
        assert!(!report.is_empty());
        assert!(!report.has_errors(), "a warning does not refuse a message");
        assert_eq!(report.reception_status(), ReceptionStatusValues::Ok);
        assert_eq!(report.diagnostic_label(), None);

        report.push(rules::FACTOR_RANGE, "/operation_mode_factor", "1.3");
        assert!(report.has_errors());
        assert_eq!(
            report.reception_status(),
            ReceptionStatusValues::InvalidContent
        );
        assert_eq!(
            report.diagnostic_label().unwrap(),
            "S2-NUM-004 at /operation_mode_factor: 1.3"
        );
    }

    #[test]
    fn severity_comes_from_the_catalogue_not_the_call_site() {
        let mut report = Report::new();
        report.push(rules::PERCENTILE_ORDER, "", "x");
        report.push(rules::NON_FINITE, "", "y");
        assert_eq!(report.violations()[0].severity, Severity::Warning);
        assert_eq!(report.violations()[1].severity, Severity::Error);
    }

    #[test]
    fn contiguity_tolerates_arithmetic_but_not_a_gap() {
        // A boundary computed rather than written: 100/3 from one side, the sum of two
        // thirds from the other. They are the same number to any reader and a few bits
        // apart to a machine, and a rule that calls that a gap reports floating point.
        let third = 100.0 / 3.0;
        let computed = 100.0 - (100.0 / 3.0) - (100.0 / 3.0);
        let mut out = Report::new();
        check_contiguous(
            &[
                NumberRange::new(0.0, third),
                NumberRange::new(100.0 - third - computed, 100.0),
            ],
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            "bands",
            "/elements",
            &mut out,
        );
        assert!(out.is_empty(), "{out}");

        // A gap of a thousandth of a unit is still a gap.
        let mut out = Report::new();
        check_contiguous(
            &[NumberRange::new(0.0, 50.0), NumberRange::new(50.001, 100.0)],
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            "bands",
            "/elements",
            &mut out,
        );
        assert!(out.contains(rules::FRBC_ELEMENTS_CONTIGUOUS));
    }

    #[test]
    fn contiguity_accepts_touching_bands_and_rejects_gaps_and_overlaps() {
        let mut out = Report::new();
        check_contiguous(
            &[NumberRange::new(0.0, 50.0), NumberRange::new(50.0, 100.0)],
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            "bands",
            "/elements",
            &mut out,
        );
        assert!(out.is_empty(), "{out}");

        let mut out = Report::new();
        check_contiguous(
            &[NumberRange::new(0.0, 40.0), NumberRange::new(50.0, 100.0)],
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            "bands",
            "/elements",
            &mut out,
        );
        assert!(out.contains(rules::FRBC_ELEMENTS_CONTIGUOUS));
        assert_eq!(out.violations()[0].path, "/elements/1");

        let mut out = Report::new();
        check_contiguous(
            &[NumberRange::new(0.0, 60.0), NumberRange::new(50.0, 100.0)],
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            "bands",
            "/elements",
            &mut out,
        );
        assert!(out.contains(rules::FRBC_ELEMENTS_CONTIGUOUS));
    }

    #[test]
    fn array_bounds_come_from_the_generated_table() {
        let mut out = Report::new();
        check_array("PowerForecast", "elements", 0, "", &mut out);
        assert!(out.contains(rules::ARRAY_BOUNDS));
        assert!(out.violations()[0].message.contains("1..=288"));

        let mut out = Report::new();
        check_array("PowerForecast", "elements", 288, "", &mut out);
        assert!(out.is_empty());

        let mut out = Report::new();
        check_array("PowerForecast", "elements", 289, "", &mut out);
        assert!(out.contains(rules::ARRAY_BOUNDS));
    }

    #[test]
    fn percentiles_must_ascend() {
        let mut out = Report::new();
        check_percentiles(
            &[
                ("lower_limit", Some(0.0)),
                ("lower_95", Some(1.0)),
                ("lower_68", None),
                ("expected", Some(2.0)),
                ("upper_68", None),
                ("upper_95", Some(3.0)),
                ("upper_limit", Some(4.0)),
            ],
            "/v",
            &mut out,
        );
        assert!(out.is_empty(), "{out}");

        let mut out = Report::new();
        check_percentiles(
            &[
                ("lower_limit", Some(0.0)),
                ("lower_95", None),
                ("lower_68", None),
                ("expected", Some(2.0)),
                ("upper_68", Some(1.0)),
                ("upper_95", None),
                ("upper_limit", None),
            ],
            "/v",
            &mut out,
        );
        assert!(out.contains(rules::PERCENTILE_ORDER));
        assert_eq!(out.violations()[0].path, "/v/upper_68");
    }

    #[test]
    fn skew_is_about_the_past_and_the_horizon_is_about_the_future() {
        use crate::types::common::{PowerMeasurement, PowerValue};
        use alloc::vec;

        let now: Timestamp = "2024-01-01T12:00:00Z".parse().expect("a timestamp");
        let ctx = Context {
            now: Some(now),
            skew_tolerance: Duration::from_secs(30),
            ..Context::empty()
        };
        let measured_at = |at: &str| PowerMeasurement {
            message_id: Id::new_const("m1"),
            measurement_timestamp: at.parse().expect("a timestamp"),
            values: vec![PowerValue::new(CommodityQuantity::ElectricPowerL1, 1.0)],
        };

        // A measurement says something has already happened, so half a minute of skew is
        // tolerated and a minute is not — which is what the knob is for.
        assert!(
            measured_at("2024-01-01T12:00:29Z")
                .validate(&ctx)
                .is_empty()
        );
        assert!(
            measured_at("2024-01-01T12:01:00Z")
                .validate(&ctx)
                .contains(rules::TIMESTAMP_SKEW)
        );
        // The past is never skew: "in the past means already".
        assert!(
            measured_at("2020-01-01T00:00:00Z")
                .validate(&ctx)
                .is_empty()
        );
        // And the knob really is one.
        let patient = Context {
            skew_tolerance: Duration::from_secs(3600),
            ..ctx
        };
        assert!(
            measured_at("2024-01-01T12:01:00Z")
                .validate(&patient)
                .is_empty()
        );

        // A *scheduled* instant is allowed to be in the future — that is what it is for —
        // and only an absurd one is worth a word.
        let mut out = Report::new();
        check_scheduled(
            "2024-06-01T00:00:00Z".parse().expect("a timestamp"),
            "/valid_from",
            &ctx,
            &mut out,
        );
        assert!(out.is_empty(), "five months ahead is a plan, not a fault");
        let mut out = Report::new();
        check_scheduled(
            "2030-01-01T00:00:00Z".parse().expect("a timestamp"),
            "/valid_from",
            &ctx,
            &mut out,
        );
        assert!(out.contains(rules::TIMESTAMP_SKEW));
    }

    #[test]
    fn mixing_phase_conventions_is_reported_once() {
        let mut out = Report::new();
        check_unique_quantities(
            [
                CommodityQuantity::ElectricPowerL1,
                CommodityQuantity::ElectricPower3PhaseSymmetric,
            ],
            "/values",
            &mut out,
        );
        assert!(out.contains(rules::PHASE_EXCLUSIVITY));
        assert!(!out.has_errors(), "E12 is undocumented, so it is a warning");
    }

    #[test]
    fn a_repeated_quantity_is_an_error() {
        let mut out = Report::new();
        check_unique_quantities(
            [
                CommodityQuantity::ElectricPowerL1,
                CommodityQuantity::ElectricPowerL1,
            ],
            "/values",
            &mut out,
        );
        assert!(out.contains(rules::ONE_VALUE_PER_QUANTITY));
        assert!(out.has_errors());
    }
}
