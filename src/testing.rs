//! A harness for driving both engines against each other, on a virtual clock.
//!
//! Because the sessions are deterministic and take `now` as a parameter, a whole
//! conversation — handshake, control-type selection, a description, an instruction, a
//! lost acknowledgement, a timer expiring — is an ordinary unit test that runs in
//! microseconds. [`Conversation`] wires a [`RmSession`] and a [`CemSession`] back to
//! back through an in-memory pipe and records everything that crosses it.
//!
//! **Every message crosses the wire format on the way**, so what is under test is the
//! encoding as well as the logic.
//!
//! ```
//! use s2_kit::prelude::*;
//! use s2_kit::testing::Conversation;
//!
//! let mut c = Conversation::battery();
//! c.open();
//! assert!(matches!(c.cem.state(), SessionState::Connected));
//! assert!(c.cem.details().is_some(), "the resource described itself");
//! ```

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::message::{Message, MessageKind};
use crate::session::{
    Analyzer, CemConfig, CemEvent, CemSession, Instructed, RmConfig, RmEvent, RmSession,
};
use crate::types::common::{
    Commodity, CommodityQuantity, ControlType, NumberRange, PowerRange, ReceptionStatusValues,
    ResourceManagerDetails, Role, RoleType, Timer, Transition,
};
use crate::types::{Duration, Id, Timestamp, frbc, pebc};
use crate::validate::Severity;

/// Which way a message went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the Customer Energy Manager to the Resource Manager.
    CemToRm,
    /// From the Resource Manager to the Customer Energy Manager.
    RmToCem,
}

impl Direction {
    /// The short form used in a transcript file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Direction::CemToRm => "cem>rm",
            Direction::RmToCem => "rm>cem",
        }
    }
}

/// One line of a `.s2log` transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    /// When it was sent, on the conversation's clock.
    pub at: Timestamp,
    /// Which way.
    pub direction: Direction,
    /// What it was.
    pub kind: MessageKind,
    /// The bytes.
    pub text: String,
}

impl Direction {
    /// The role that sent a message going this way.
    #[must_use]
    pub const fn sender(self) -> crate::types::common::EnergyManagementRole {
        match self {
            Direction::CemToRm => crate::types::common::EnergyManagementRole::Cem,
            Direction::RmToCem => crate::types::common::EnergyManagementRole::Rm,
        }
    }

    /// The direction a transcript line's `dir` field names.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cem>rm" => Some(Direction::CemToRm),
            "rm>cem" => Some(Direction::RmToCem),
            _ => None,
        }
    }
}

impl TranscriptEntry {
    /// The entry as one line of JSON Lines.
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        format!(
            r#"{{"t":"{}","dir":"{}","msg":{}}}"#,
            self.at,
            self.direction.as_str(),
            self.text
        )
    }

    /// Read one line of a `.s2log` back.
    ///
    /// The inverse of [`to_jsonl`](Self::to_jsonl). `None` for a line that is not one — a
    /// blank line, a comment, or a record from some other tool.
    #[must_use]
    pub fn from_jsonl(line: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let at = value.get("t")?.as_str()?.parse().ok()?;
        let direction = Direction::parse(value.get("dir")?.as_str()?)?;
        let text = value.get("msg")?.to_string();
        // The kind is derived rather than stored: a transcript records what crossed the
        // wire, and a `message_type` the reader does not know is exactly the thing a
        // replay must be able to report rather than refuse to load.
        let kind = crate::codec::peek(&text).ok().and_then(|p| p.kind)?;
        Some(Self {
            at,
            direction,
            kind,
            text,
        })
    }
}

/// Read a whole `.s2log`, skipping anything that is not a transcript line.
#[must_use]
pub fn read_s2log(text: &str) -> Vec<TranscriptEntry> {
    text.lines()
        .filter_map(TranscriptEntry::from_jsonl)
        .collect()
}

/// A Resource Manager and a Customer Energy Manager, talking to each other.
pub struct Conversation {
    /// The resource's side.
    pub rm: RmSession,
    /// The manager's side.
    pub cem: CemSession,
    /// The virtual clock.
    pub now: Timestamp,
    transcript: Vec<TranscriptEntry>,
    rm_events: Vec<RmEvent>,
    cem_events: Vec<CemEvent>,
}

impl Conversation {
    /// A conversation between the given two configurations.
    #[must_use]
    pub fn new(
        rm_config: RmConfig,
        details: ResourceManagerDetails,
        cem_config: CemConfig,
    ) -> Self {
        Self {
            rm: RmSession::new(rm_config, details),
            cem: CemSession::new(cem_config),
            now: "2024-01-01T12:00:00Z"
                .parse()
                .unwrap_or(Timestamp::UNIX_EPOCH),
            transcript: Vec::new(),
            rm_events: Vec::new(),
            cem_events: Vec::new(),
        }
    }

    /// A conversation with a 20 kWh home battery, the FRBC example implementation.
    #[must_use]
    pub fn battery() -> Self {
        Self::new(RmConfig::default(), battery_details(), CemConfig::default())
    }

    /// A conversation with a 4 kWp curtailable PV inverter, the PEBC example.
    #[must_use]
    pub fn pv() -> Self {
        Self::new(RmConfig::default(), pv_details(), CemConfig::default())
    }

    /// Open both sides and run the conversation until it goes quiet.
    pub fn open(&mut self) {
        self.rm.open(self.now);
        self.cem.open(self.now);
        self.pump();
    }

    /// Deliver everything either side wants to send, until neither has anything left.
    ///
    /// Bounded, so a session that answers its own answers cannot hang a test.
    ///
    /// # Panics
    ///
    /// When the conversation does not settle within a thousand rounds, which means one
    /// of the engines is acknowledging acknowledgements.
    #[allow(clippy::panic)] // a test harness that hangs is worse than one that fails
    pub fn pump(&mut self) {
        for _ in 0..1000 {
            let mut moved = false;
            while let Some(out) = self.rm.poll_transmit() {
                self.record(Direction::RmToCem, out.kind, &out.text);
                self.cem.handle_text(&out.text, self.now);
                moved = true;
            }
            while let Some(out) = self.cem.poll_transmit() {
                self.record(Direction::CemToRm, out.kind, &out.text);
                self.rm.handle_text(&out.text, self.now);
                moved = true;
            }
            self.drain_events();
            if !moved {
                return;
            }
        }
        panic!("the conversation did not settle: a session is answering its own answers");
    }

    fn record(&mut self, direction: Direction, kind: MessageKind, text: &str) {
        self.transcript.push(TranscriptEntry {
            at: self.now,
            direction,
            kind,
            text: text.to_string(),
        });
    }

    fn drain_events(&mut self) {
        while let Some(event) = self.rm.poll_event() {
            self.rm_events.push(event);
        }
        while let Some(event) = self.cem.poll_event() {
            self.cem_events.push(event);
        }
    }

    /// Move the clock forward, letting both sides notice.
    pub fn advance(&mut self, by: Duration) {
        self.now = self.now.checked_add(by).unwrap_or(self.now);
        self.rm.handle_timeout(self.now);
        self.cem.handle_timeout(self.now);
        self.pump();
    }

    /// Move the clock forward without delivering anything, so that a message can be
    /// dropped on the floor.
    pub fn advance_silently(&mut self, by: Duration) {
        self.now = self.now.checked_add(by).unwrap_or(self.now);
        self.rm.handle_timeout(self.now);
        self.cem.handle_timeout(self.now);
        self.drain_events();
    }

    /// Throw away everything the Resource Manager is trying to say, as a lost
    /// connection would.
    pub fn drop_rm_traffic(&mut self) {
        while self.rm.poll_transmit().is_some() {}
        self.drain_events();
    }

    /// Throw away everything the manager is trying to say.
    pub fn drop_cem_traffic(&mut self) {
        while self.cem.poll_transmit().is_some() {}
        self.drain_events();
    }

    /// Everything that has crossed the wire.
    #[must_use]
    pub fn transcript(&self) -> &[TranscriptEntry] {
        &self.transcript
    }

    /// The transcript as a `.s2log` file.
    #[must_use]
    pub fn to_s2log(&self) -> String {
        let mut out = String::new();
        for entry in &self.transcript {
            out.push_str(&entry.to_jsonl());
            out.push('\n');
        }
        out
    }

    /// Every message type that has crossed, in order.
    #[must_use]
    pub fn flow(&self) -> Vec<(Direction, MessageKind)> {
        self.transcript
            .iter()
            .map(|e| (e.direction, e.kind))
            .collect()
    }

    /// Every message type that has crossed, ignoring the acknowledgements.
    #[must_use]
    pub fn flow_without_acks(&self) -> Vec<(Direction, MessageKind)> {
        self.flow()
            .into_iter()
            .filter(|(_, kind)| *kind != MessageKind::ReceptionStatus)
            .collect()
    }

    /// Everything the Resource Manager has reported, clearing the list.
    pub fn take_rm_events(&mut self) -> Vec<RmEvent> {
        self.drain_events();
        core::mem::take(&mut self.rm_events)
    }

    /// Everything the manager has reported, clearing the list.
    pub fn take_cem_events(&mut self) -> Vec<CemEvent> {
        self.drain_events();
        core::mem::take(&mut self.cem_events)
    }

    /// The first instruction the Resource Manager was handed, if any.
    pub fn first_instruction(&mut self) -> Option<Instructed> {
        self.drain_events();
        self.rm_events.iter().find_map(|e| match e {
            RmEvent::Instruction(i) => Some((**i).clone()),
            _ => None,
        })
    }

    /// Whether either side refused anything.
    #[must_use]
    pub fn refusals(&self) -> Vec<String> {
        let mut out = Vec::new();
        for event in &self.rm_events {
            if let RmEvent::Refused { report, .. } = event {
                out.push(format!("RM refused: {report}"));
            }
        }
        for event in &self.cem_events {
            if let CemEvent::Refused { report, .. } = event {
                out.push(format!("CEM refused: {report}"));
            }
        }
        out
    }

    /// Assert that nothing was refused. The message names what was.
    ///
    /// # Panics
    ///
    /// When either side refused a message.
    pub fn assert_no_refusals(&self) {
        let refusals = self.refusals();
        assert!(refusals.is_empty(), "{refusals:#?}");
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The 20 kWh home battery from `s2-example-implementations`.
#[must_use]
pub fn battery_details() -> ResourceManagerDetails {
    ResourceManagerDetails {
        message_id: Id::new_const("rmd-battery"),
        resource_id: Id::new_const("acme_battery_1"),
        name: Some("Home battery".into()),
        roles: alloc::vec![Role::new(RoleType::EnergyStorage, Commodity::Electricity)],
        manufacturer: Some("ACME".into()),
        model: Some("Powerwall-ish".into()),
        serial_number: Some("123".into()),
        firmware_version: Some("v1.0".into()),
        instruction_processing_delay: Duration::from_millis(500),
        available_control_types: alloc::vec![ControlType::FillRateBasedControl],
        currency: None,
        provides_forecast: false,
        provides_power_measurement_types: alloc::vec![
            CommodityQuantity::ElectricPower3PhaseSymmetric
        ],
    }
}

/// A battery's system description: charge, idle and discharge over a 0–100 % store.
#[must_use]
pub fn battery_system(valid_from: Timestamp) -> frbc::SystemDescription {
    let mode =
        |id: &'static str, label: &str, rate: NumberRange, power: PowerRange| frbc::OperationMode {
            id: Id::new_const(id),
            diagnostic_label: Some(label.to_string()),
            elements: alloc::vec![frbc::OperationModeElement {
                fill_level_range: NumberRange::new(0.0, 100.0),
                fill_rate: rate,
                power_ranges: alloc::vec![power],
                running_costs: None,
            }],
            abnormal_condition_only: false,
        };
    let q = CommodityQuantity::ElectricPower3PhaseSymmetric;
    frbc::SystemDescription {
        message_id: Id::new_const("frbc-system-1"),
        valid_from,
        actuators: alloc::vec![frbc::ActuatorDescription {
            id: Id::new_const("actuator1"),
            diagnostic_label: Some("Battery inverter".into()),
            supported_commodities: alloc::vec![Commodity::Electricity],
            operation_modes: alloc::vec![
                mode(
                    "idle",
                    "Idle",
                    NumberRange::exactly(0.0),
                    PowerRange::exactly(0.0, q)
                ),
                mode(
                    "charge",
                    "Charging",
                    NumberRange::new(0.0, 0.0025),
                    PowerRange::new(0.0, 5000.0, q),
                ),
                mode(
                    "discharge",
                    "Discharging",
                    NumberRange::new(0.0, -0.0025),
                    PowerRange::new(0.0, -5000.0, q),
                ),
            ],
            transitions: alloc::vec![
                Transition::simple(
                    Id::new_const("t1"),
                    Id::new_const("idle"),
                    Id::new_const("charge")
                ),
                Transition::simple(
                    Id::new_const("t2"),
                    Id::new_const("charge"),
                    Id::new_const("idle")
                ),
                Transition::simple(
                    Id::new_const("t3"),
                    Id::new_const("idle"),
                    Id::new_const("discharge")
                ),
                Transition {
                    id: Id::new_const("t4"),
                    from: Id::new_const("discharge"),
                    to: Id::new_const("idle"),
                    start_timers: alloc::vec![],
                    blocking_timers: alloc::vec![Id::new_const("cooldown")],
                    transition_costs: None,
                    transition_duration: None,
                    abnormal_condition_only: false,
                },
            ],
            timers: alloc::vec![Timer {
                id: Id::new_const("cooldown"),
                diagnostic_label: Some("Minimum discharge time".into()),
                duration: Duration::from_secs(600),
            }],
        }],
        storage: frbc::StorageDescription {
            diagnostic_label: Some("Battery".into()),
            fill_level_label: Some("percentage state of charge".into()),
            provides_leakage_behaviour: false,
            provides_fill_level_target_profile: false,
            provides_usage_forecast: false,
            fill_level_range: NumberRange::new(0.0, 100.0),
        },
    }
}

/// The 2 kWp curtailable PV installation from `s2-example-implementations`.
#[must_use]
pub fn pv_details() -> ResourceManagerDetails {
    ResourceManagerDetails {
        message_id: Id::new_const("rmd-pv"),
        resource_id: Id::new_const("acme_pv_installation"),
        name: Some("Solar panels on roof".into()),
        roles: alloc::vec![Role::new(RoleType::EnergyProducer, Commodity::Electricity)],
        manufacturer: Some("ACME".into()),
        model: Some("InverterType2024".into()),
        serial_number: Some("123".into()),
        firmware_version: Some("v1.0".into()),
        instruction_processing_delay: Duration::from_millis(5000),
        available_control_types: alloc::vec![ControlType::PowerEnvelopeBasedControl],
        currency: None,
        provides_forecast: true,
        provides_power_measurement_types: alloc::vec![CommodityQuantity::ElectricPowerL1],
    }
}

/// The PV installation's constraints: curtail freely between −4 kW and zero.
#[must_use]
pub fn pv_constraints(valid_from: Timestamp) -> pebc::PowerConstraints {
    pebc::PowerConstraints {
        message_id: Id::new_const("pebc-constraints-1"),
        id: Id::new_const("powerConstraint1"),
        valid_from,
        valid_until: None,
        consequence_type: pebc::PowerEnvelopeConsequenceType::Vanish,
        allowed_limit_ranges: alloc::vec![
            pebc::AllowedLimitRange {
                commodity_quantity: CommodityQuantity::ElectricPowerL1,
                limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                range_boundary: NumberRange::new(-4000.0, 0.0),
                abnormal_condition_only: false,
            },
            pebc::AllowedLimitRange {
                commodity_quantity: CommodityQuantity::ElectricPowerL1,
                limit_type: pebc::PowerEnvelopeLimitType::UpperLimit,
                range_boundary: NumberRange::new(0.0, 0.0),
                abnormal_condition_only: false,
            },
        ],
    }
}

/// An FRBC instruction for the battery fixture.
#[must_use]
pub fn charge(id: &'static str, factor: f64, at: Timestamp) -> frbc::Instruction {
    frbc::Instruction {
        message_id: Id::new_const("frbc-instr-msg"),
        id: Id::new_const(id),
        actuator_id: Id::new_const("actuator1"),
        operation_mode: Id::new_const("charge"),
        operation_mode_factor: factor,
        execution_time: at,
        abnormal_condition: false,
    }
}

// ---------------------------------------------------------------------------
// Replaying a recorded conversation
// ---------------------------------------------------------------------------

/// Something a recorded conversation did that this crate would not have done.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Finding {
    /// A rule fired on a message the transcript carries.
    Violation {
        /// Which line of the transcript, counting from one.
        line: usize,
        /// Which way it went.
        direction: Direction,
        /// What it was.
        kind: MessageKind,
        /// What was wrong with it.
        violation: crate::validate::Violation,
    },
    /// The peer answered with a different `ReceptionStatus` than this crate would have.
    ///
    /// The interop finding: two readings of the standard, in one line, with the rule that
    /// produced this side's named.
    StatusDiffers {
        /// Which line carried the message that was answered.
        line: usize,
        /// Which way that message went.
        direction: Direction,
        /// What it was.
        kind: MessageKind,
        /// What the transcript records the peer answering.
        recorded: ReceptionStatusValues,
        /// What this crate would answer.
        expected: ReceptionStatusValues,
        /// Why — the first error, rule identifier first. `None` when this crate would
        /// have accepted the message and the peer did not.
        because: Option<String>,
    },
    /// A message that must be answered was never answered.
    ///
    /// `S2J` requires a `ReceptionStatus` for every message except a `ReceptionStatus`;
    /// forgetting one is the deadlock `[s2-json #22]` describes, and a transcript is the
    /// only place it shows.
    Unanswered {
        /// Which line.
        line: usize,
        /// Which way it went.
        direction: Direction,
        /// What it was.
        kind: MessageKind,
        /// Its identifier, which the missing answer should have named.
        message_id: Id,
    },
    /// A `ReceptionStatus` answering a message the transcript does not contain.
    ///
    /// Either the recording is incomplete or the peer answered something nobody sent.
    UnmatchedAnswer {
        /// Which line.
        line: usize,
        /// Which way it went.
        direction: Direction,
        /// What it claimed to answer.
        subject: Id,
    },
}

impl Finding {
    /// Which line of the transcript this is about.
    #[must_use]
    pub const fn line(&self) -> usize {
        match self {
            Finding::Violation { line, .. }
            | Finding::StatusDiffers { line, .. }
            | Finding::Unanswered { line, .. }
            | Finding::UnmatchedAnswer { line, .. } => *line,
        }
    }

    /// Whether this finding means the conversation was not conforming.
    ///
    /// A warning-severity violation is not: the standard is ambiguous there, and another
    /// implementation is entitled to read the silence differently.
    #[must_use]
    pub fn is_error(&self) -> bool {
        match self {
            Finding::Violation { violation, .. } => violation.severity == Severity::Error,
            _ => true,
        }
    }
}

impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Finding::Violation {
                line,
                direction,
                kind,
                violation,
            } => write!(f, "{line:>4} {} {kind}: {violation}", direction.as_str()),
            Finding::StatusDiffers {
                line,
                direction,
                kind,
                recorded,
                expected,
                because,
            } => {
                write!(
                    f,
                    "{line:>4} {} {kind}: answered {recorded:?}, s2-kit would answer {expected:?}",
                    direction.as_str()
                )?;
                match because {
                    Some(why) => write!(f, " ({why})"),
                    None => Ok(()),
                }
            }
            Finding::Unanswered {
                line,
                direction,
                kind,
                message_id,
            } => write!(
                f,
                "{line:>4} {} {kind}: never answered; no ReceptionStatus names {message_id}",
                direction.as_str()
            ),
            Finding::UnmatchedAnswer {
                line,
                direction,
                subject,
            } => write!(
                f,
                "{line:>4} {} ReceptionStatus: answers {subject}, which no line sent",
                direction.as_str()
            ),
        }
    }
}

/// What a replay found, and what it saw.
#[derive(Debug, Clone, Default)]
pub struct ReplayReport {
    /// Everything worth saying, in transcript order.
    pub findings: Vec<Finding>,
    /// How many lines were read.
    pub lines: usize,
    /// How many of them needed an answer and got one.
    pub answered: usize,
}

impl ReplayReport {
    /// Whether the conversation conformed.
    #[must_use]
    pub fn is_conforming(&self) -> bool {
        !self.findings.iter().any(Finding::is_error)
    }
}

impl core::fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for finding in &self.findings {
            writeln!(f, "{finding}")?;
        }
        write!(
            f,
            "{} line(s), {} answered: {}",
            self.lines,
            self.answered,
            if self.is_conforming() { "ok" } else { "FAILED" }
        )
    }
}

/// Replay a recorded conversation and say where it departs from the standard.
///
/// Three questions, and only the first is one a message-at-a-time validator can answer:
///
/// 1. **Is each message legal, given everything before it?** An [`Analyzer`] keeps one
///    registry fed from both directions, so an instruction naming an actuator nobody
///    described is a finding rather than a blind spot.
/// 2. **Did the peer answer as this crate would?** A disagreement is the interop report.
/// 3. **Was every message answered at all?**
///
/// The clock is the transcript's own, so a validity window or a timer is replayed where it
/// actually fell.
///
/// ```
/// use s2_kit::testing::{read_s2log, replay};
///
/// let log = "\
/// {\"t\":\"2024-01-01T12:00:00Z\",\"dir\":\"rm>cem\",\"msg\":{\"message_type\":\"FRBC.StorageStatus\",\"message_id\":\"m1\",\"present_fill_level\":52.0}}\n";
/// let report = replay(&read_s2log(log));
/// // Nobody answered it — the kind of thing only a transcript shows.
/// assert_eq!(report.lines, 1);
/// assert!(!report.is_conforming());
/// ```
#[must_use]
pub fn replay(entries: &[TranscriptEntry]) -> ReplayReport {
    replay_with(entries, Analyzer::new())
}

/// As [`replay`], with the observer configured — a different wire profile, strict
/// decoding, an S2 Connect session where a handshake would be redundant.
#[must_use]
pub fn replay_with(entries: &[TranscriptEntry], mut analyzer: Analyzer) -> ReplayReport {
    let mut report = ReplayReport {
        lines: entries.len(),
        ..ReplayReport::default()
    };

    // What each side actually answered, by the identifier it named. Built first, because
    // an answer may legitimately arrive several lines after the message it is about.
    let mut answers: Vec<(Direction, Id, ReceptionStatusValues)> = Vec::new();
    for entry in entries {
        if entry.kind == MessageKind::ReceptionStatus
            && let Ok(Message::ReceptionStatus(status)) = crate::codec::decode(&entry.text)
        {
            answers.push((entry.direction, status.subject_message_id, status.status));
        }
    }

    for (index, entry) in entries.iter().enumerate() {
        let line = index + 1;
        let observed = analyzer.observe(entry.direction.sender(), &entry.text, entry.at);

        for violation in observed.report.violations() {
            report.findings.push(Finding::Violation {
                line,
                direction: entry.direction,
                kind: entry.kind,
                violation: violation.clone(),
            });
        }

        let Some(message) = &observed.message else {
            continue;
        };

        if let Message::ReceptionStatus(status) = message {
            // An answer to a message this transcript does not contain: either the
            // recording is incomplete, or the peer answered something nobody sent. The
            // nil identifier is the one exception — it is what the ecosystem sends when
            // the message had no readable id at all (erratum E10).
            let sent = entries.iter().any(|other| {
                other.direction != entry.direction
                    && crate::codec::peek(&other.text)
                        .ok()
                        .and_then(|p| p.message_id)
                        == Some(status.subject_message_id)
            });
            if !sent && status.subject_message_id != Id::NIL {
                report.findings.push(Finding::UnmatchedAnswer {
                    line,
                    direction: entry.direction,
                    subject: status.subject_message_id,
                });
            }
            continue;
        }

        let Some(message_id) = message.id() else {
            continue;
        };
        // The answer must come back the other way.
        let recorded = answers
            .iter()
            .find(|(direction, subject, _)| *direction != entry.direction && *subject == message_id)
            .map(|(_, _, status)| *status);

        match recorded {
            None => report.findings.push(Finding::Unanswered {
                line,
                direction: entry.direction,
                kind: entry.kind,
                message_id,
            }),
            Some(recorded) => {
                report.answered += 1;
                // `TEMPORARY_ERROR` and `PERMANENT_ERROR` are about the *receiver* — a
                // storage that was unavailable, a device that was offline — and no
                // observer can know that. They are never a disagreement.
                let receiver_specific = matches!(
                    recorded,
                    ReceptionStatusValues::TemporaryError | ReceptionStatusValues::PermanentError
                );
                if recorded != observed.status && !receiver_specific {
                    report.findings.push(Finding::StatusDiffers {
                        line,
                        direction: entry.direction,
                        kind: entry.kind,
                        recorded,
                        expected: observed.status,
                        because: observed.diagnostic_label(),
                    });
                }
            }
        }
    }

    report.findings.sort_by_key(Finding::line);
    report
}
