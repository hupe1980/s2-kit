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
    Commodity, CommodityQuantity, ControlType, Currency, EnergyManagementRole, Handshake,
    HandshakeResponse, InstructionStatus, InstructionStatusUpdate, NumberRange, PowerForecast,
    PowerForecastElement, PowerForecastValue, PowerMeasurement, PowerRange, PowerValue,
    ReceptionStatus, ReceptionStatusValues, ResourceManagerDetails, RevokableObjects, RevokeObject,
    Role, RoleType, SelectControlType, SessionRequest, SessionRequestType, Timer, Transition,
};
use crate::types::{Duration, Id, ProtocolVersion, Timestamp, ddbc, frbc, ombc, pebc, ppbc};
use crate::validate::Severity;

/// Which way a message went.
///
/// Ordered so that a transcript index can be sorted and binary-searched by
/// `(direction, message_id)`; the order itself means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// From the Customer Energy Manager to the Resource Manager.
    CemToRm,
    /// From the Resource Manager to the Customer Energy Manager.
    RmToCem,
}

impl Direction {
    /// The other way.
    ///
    /// An acknowledgement always travels against the message it answers, which is the
    /// one thing every lookup in [`replay`] needs.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Direction::CemToRm => Direction::RmToCem,
            Direction::RmToCem => Direction::CemToRm,
        }
    }

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

    // Two indexes, both built in one pass, because the alternative is quadratic and this
    // is the tool a field capture is fed to. A transcript of a hundred thousand lines is
    // an ordinary thing to be handed after a week of trouble.
    //
    // * what each side answered, by the identifier it named — an answer may legitimately
    //   arrive several lines after the message it is about;
    // * which identifiers were actually *sent*, and in which direction, so that an answer
    //   naming a message nobody sent can be spotted without re-peeking every other line.
    let mut answers: Vec<(Direction, Id, ReceptionStatusValues)> = Vec::new();
    let mut sent: Vec<(Direction, Id)> = Vec::new();
    for entry in entries {
        if entry.kind == MessageKind::ReceptionStatus {
            if let Ok(Message::ReceptionStatus(status)) = crate::codec::decode(&entry.text) {
                answers.push((entry.direction, status.subject_message_id, status.status));
            }
            continue;
        }
        if let Ok(Some(id)) = crate::codec::peek(&entry.text).map(|p| p.message_id) {
            sent.push((entry.direction, id));
        }
    }
    answers.sort_unstable_by_key(|(direction, subject, _)| (*direction, *subject));
    sent.sort_unstable();

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
            let was_sent = sent
                .binary_search(&(entry.direction.other(), status.subject_message_id))
                .is_ok();
            if !was_sent && status.subject_message_id != Id::NIL {
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
            .binary_search_by_key(&(entry.direction.other(), message_id), |(d, s, _)| (*d, *s))
            .ok()
            .and_then(|i| answers.get(i))
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

// ---------------------------------------------------------------------------
// One of everything, with every optional field populated
// ---------------------------------------------------------------------------

fn id(s: &str) -> Id {
    Id::parse(s).unwrap_or(Id::NIL)
}

fn at() -> Timestamp {
    Timestamp::from_unix(1_724_508_922, 0)
}

fn q() -> CommodityQuantity {
    CommodityQuantity::ElectricPowerL1
}

fn number_range() -> NumberRange {
    NumberRange::new(0.0, 100.0)
}

fn power_range() -> PowerRange {
    PowerRange::new(0.0, 2000.0, q())
}

fn forecast_value() -> PowerForecastValue {
    PowerForecastValue {
        value_upper_limit: Some(1000.0),
        value_upper_95ppr: Some(960.0),
        value_upper_68ppr: Some(800.0),
        value_expected: 545.1,
        value_lower_68ppr: Some(340.0),
        value_lower_95ppr: Some(100.0),
        value_lower_limit: Some(0.0),
        commodity_quantity: q(),
    }
}

fn timer() -> Timer {
    Timer {
        id: id("timer0"),
        diagnostic_label: Some("Minimum run time".into()),
        duration: Duration::from_secs(7200),
    }
}

fn transition(from: &str, to: &str) -> Transition {
    Transition {
        id: id("trans0"),
        from: id(from),
        to: id(to),
        start_timers: vec![id("timer0")],
        blocking_timers: vec![id("timer0")],
        transition_costs: Some(0.0),
        transition_duration: Some(Duration::from_secs(3)),
        abnormal_condition_only: false,
    }
}

fn frbc_actuator() -> frbc::ActuatorDescription {
    frbc::ActuatorDescription {
        id: id("actuator1"),
        diagnostic_label: Some("heat pump".into()),
        supported_commodities: vec![Commodity::Electricity],
        operation_modes: vec![
            frbc::OperationMode {
                id: id("om0"),
                diagnostic_label: Some("running".into()),
                elements: vec![frbc::OperationModeElement {
                    fill_level_range: number_range(),
                    fill_rate: NumberRange::new(0.002_09, 0.008_35),
                    power_ranges: vec![power_range()],
                    running_costs: Some(NumberRange::new(0.0, 0.0)),
                }],
                abnormal_condition_only: false,
            },
            frbc::OperationMode {
                id: id("om1"),
                diagnostic_label: Some("Off".into()),
                elements: vec![frbc::OperationModeElement {
                    fill_level_range: number_range(),
                    fill_rate: NumberRange::exactly(0.0),
                    power_ranges: vec![PowerRange::exactly(0.0, q())],
                    running_costs: None,
                }],
                abnormal_condition_only: false,
            },
        ],
        transitions: vec![transition("om0", "om1")],
        timers: vec![timer()],
    }
}

fn ddbc_actuator() -> ddbc::ActuatorDescription {
    ddbc::ActuatorDescription {
        id: id("actuator1"),
        diagnostic_label: Some("hybrid heat pump".into()),
        supported_commodities: vec![Commodity::Electricity, Commodity::Gas],
        operation_modes: vec![ddbc::OperationMode {
            id: id("om0"),
            diagnostic_label: Some("electric".into()),
            power_ranges: vec![power_range()],
            supply_range: NumberRange::new(0.0, 6000.0),
            running_costs: Some(NumberRange::new(0.0, 0.0)),
            abnormal_condition_only: false,
        }],
        transitions: vec![transition("om0", "om0")],
        timers: vec![timer()],
    }
}

/// One of every message, with **every optional field set**.
///
/// The corpus three different checks are run over, and the reason it lives here rather
/// than in one of them: it is the answer to "show me every message this crate can
/// produce", which is a thing a consumer writing its own tests wants as much as this
/// crate does.
///
/// * `tests/model_matches_schema.rs` validates each one against the official JSON schema
///   and requires every component the schema defines to be reachable from one of them;
/// * `tests/interop_s2energy.rs` requires an independent implementation to read them;
/// * a consumer can use it to exercise its own handler over the whole message space.
///
/// Identifiers are short and readable (`actuator1`, `om0`) rather than UUIDs, exactly as
/// the standard's own worked examples write them — `S2J schemas/ID` is a pattern, not a
/// UUID (erratum E1).
#[must_use]
pub fn every_message() -> Vec<Message> {
    let versions = vec![ProtocolVersion::new("1.0.0")];
    vec![
        Handshake {
            message_id: id("m1"),
            role: EnergyManagementRole::Rm,
            supported_protocol_versions: Some(versions),
        }
        .into(),
        HandshakeResponse {
            message_id: id("m1"),
            selected_protocol_version: ProtocolVersion::new("1.0.0"),
        }
        .into(),
        ResourceManagerDetails {
            message_id: id("m1"),
            resource_id: id("acme_heat_pump"),
            name: Some("my_heat_pump".into()),
            roles: vec![Role::new(RoleType::EnergyConsumer, Commodity::Electricity)],
            manufacturer: Some("ACME".into()),
            model: Some("HeatPump2000".into()),
            serial_number: Some("123".into()),
            firmware_version: Some("v1.0".into()),
            instruction_processing_delay: Duration::from_millis(10_000),
            available_control_types: vec![ControlType::FillRateBasedControl],
            currency: Some(Currency::Eur),
            provides_forecast: true,
            provides_power_measurement_types: vec![q()],
        }
        .into(),
        SelectControlType {
            message_id: id("m1"),
            control_type: ControlType::FillRateBasedControl,
        }
        .into(),
        SessionRequest {
            message_id: id("m1"),
            request: SessionRequestType::Terminate,
            diagnostic_label: Some("shutting down".into()),
        }
        .into(),
        ReceptionStatus {
            subject_message_id: id("m1"),
            status: ReceptionStatusValues::Ok,
            diagnostic_label: Some("Processed okay.".into()),
        }
        .into(),
        InstructionStatusUpdate {
            message_id: id("m1"),
            instruction_id: id("instr0"),
            status_type: InstructionStatus::Succeeded,
            timestamp: at(),
        }
        .into(),
        PowerMeasurement {
            message_id: id("m1"),
            measurement_timestamp: at(),
            values: vec![PowerValue::new(q(), 510.6)],
        }
        .into(),
        PowerForecast {
            message_id: id("m1"),
            start_time: at(),
            elements: vec![PowerForecastElement {
                duration: Duration::from_millis(3_600_000),
                power_values: vec![forecast_value()],
            }],
        }
        .into(),
        RevokeObject {
            message_id: id("m1"),
            object_type: RevokableObjects::FrbcSystemDescription,
            object_id: id("sd1"),
        }
        .into(),
        // --- PEBC ---
        pebc::PowerConstraints {
            message_id: id("m1"),
            id: id("powerConstraint1"),
            valid_from: at(),
            valid_until: Some(at()),
            consequence_type: pebc::PowerEnvelopeConsequenceType::Vanish,
            allowed_limit_ranges: vec![
                pebc::AllowedLimitRange {
                    commodity_quantity: q(),
                    limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                    range_boundary: NumberRange::new(-4000.0, 0.0),
                    abnormal_condition_only: false,
                },
                pebc::AllowedLimitRange {
                    commodity_quantity: q(),
                    limit_type: pebc::PowerEnvelopeLimitType::UpperLimit,
                    range_boundary: NumberRange::new(0.0, 0.0),
                    abnormal_condition_only: false,
                },
            ],
        }
        .into(),
        pebc::EnergyConstraint {
            message_id: id("m1"),
            id: id("energyconstraint1"),
            valid_from: at(),
            valid_until: at(),
            upper_average_power: 3000.0,
            lower_average_power: 1000.0,
            commodity_quantity: q(),
        }
        .into(),
        pebc::Instruction {
            message_id: id("m1"),
            id: id("envelope1"),
            execution_time: at(),
            abnormal_condition: false,
            power_constraints_id: id("powerConstraint1"),
            power_envelopes: vec![pebc::PowerEnvelope {
                id: id("pe_xxx"),
                commodity_quantity: q(),
                power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                    Duration::from_millis(3_600_000),
                    -2000.0,
                    0.0,
                )],
            }],
        }
        .into(),
        // --- PPBC ---
        ppbc::PowerProfileDefinition {
            message_id: id("m1"),
            id: id("profile1"),
            start_time: at(),
            end_time: at(),
            power_sequence_containers: vec![ppbc::PowerSequenceContainer {
                id: id("c1"),
                power_sequences: vec![ppbc::PowerSequence {
                    id: id("s1"),
                    elements: vec![ppbc::PowerSequenceElement {
                        duration: Duration::from_secs(600),
                        power_values: vec![forecast_value()],
                    }],
                    is_interruptible: true,
                    max_pause_before: Some(Duration::from_secs(300)),
                    abnormal_condition_only: false,
                }],
            }],
        }
        .into(),
        ppbc::PowerProfileStatus {
            message_id: id("m1"),
            sequence_container_status: vec![ppbc::PowerSequenceContainerStatus {
                power_profile_id: id("profile1"),
                sequence_container_id: id("c1"),
                selected_sequence_id: Some(id("s1")),
                progress: Some(Duration::from_secs(60)),
                status: ppbc::PowerSequenceStatus::Executing,
            }],
        }
        .into(),
        ppbc::ScheduleInstruction {
            message_id: id("m1"),
            id: id("instr0"),
            power_profile_id: id("profile1"),
            sequence_container_id: id("c1"),
            power_sequence_id: id("s1"),
            execution_time: at(),
            abnormal_condition: false,
        }
        .into(),
        ppbc::StartInterruptionInstruction {
            message_id: id("m1"),
            id: id("instr1"),
            power_profile_id: id("profile1"),
            sequence_container_id: id("c1"),
            power_sequence_id: id("s1"),
            execution_time: at(),
            abnormal_condition: false,
        }
        .into(),
        ppbc::EndInterruptionInstruction {
            message_id: id("m1"),
            id: id("instr2"),
            power_profile_id: id("profile1"),
            sequence_container_id: id("c1"),
            power_sequence_id: id("s1"),
            execution_time: at(),
            abnormal_condition: false,
        }
        .into(),
        // --- OMBC ---
        ombc::SystemDescription {
            message_id: id("m1"),
            valid_from: at(),
            operation_modes: vec![ombc::OperationMode {
                id: id("om0"),
                diagnostic_label: Some("Off".into()),
                power_ranges: vec![power_range()],
                running_costs: Some(NumberRange::new(0.0, 0.0)),
                abnormal_condition_only: false,
            }],
            transitions: vec![transition("om0", "om0")],
            timers: vec![timer()],
        }
        .into(),
        ombc::Status {
            message_id: id("m1"),
            active_operation_mode_id: id("om0"),
            operation_mode_factor: 0.5,
            previous_operation_mode_id: Some(id("om1")),
            transition_timestamp: Some(at()),
        }
        .into(),
        ombc::TimerStatus {
            message_id: id("m1"),
            timer_id: id("timer0"),
            finished_at: at(),
        }
        .into(),
        ombc::Instruction {
            message_id: id("m1"),
            id: id("instr0"),
            execution_time: at(),
            operation_mode_id: id("om0"),
            operation_mode_factor: 1.0,
            abnormal_condition: false,
        }
        .into(),
        // --- FRBC ---
        frbc::SystemDescription {
            message_id: id("m1"),
            valid_from: at(),
            actuators: vec![frbc_actuator()],
            storage: frbc::StorageDescription {
                diagnostic_label: Some("DHW Buffer".into()),
                fill_level_label: Some("temperature in Celsius".into()),
                provides_leakage_behaviour: true,
                provides_fill_level_target_profile: true,
                provides_usage_forecast: true,
                fill_level_range: number_range(),
            },
        }
        .into(),
        frbc::StorageStatus {
            message_id: id("m1"),
            present_fill_level: 52.0,
        }
        .into(),
        frbc::ActuatorStatus {
            message_id: id("m1"),
            actuator_id: id("actuator1"),
            active_operation_mode_id: id("om0"),
            operation_mode_factor: 0.0,
            previous_operation_mode_id: Some(id("om1")),
            transition_timestamp: Some(at()),
        }
        .into(),
        frbc::TimerStatus {
            message_id: id("m1"),
            timer_id: id("timer0"),
            actuator_id: id("actuator1"),
            finished_at: at(),
        }
        .into(),
        frbc::LeakageBehaviour {
            message_id: id("m1"),
            valid_from: at(),
            elements: vec![frbc::LeakageBehaviourElement {
                fill_level_range: number_range(),
                leakage_rate: 0.000_05,
            }],
        }
        .into(),
        frbc::UsageForecast {
            message_id: id("m1"),
            start_time: at(),
            elements: vec![frbc::UsageForecastElement {
                duration: Duration::from_secs(900),
                usage_rate_upper_limit: Some(1.0),
                usage_rate_upper_95ppr: Some(0.9),
                usage_rate_upper_68ppr: Some(0.8),
                usage_rate_expected: 0.5,
                usage_rate_lower_68ppr: Some(0.4),
                usage_rate_lower_95ppr: Some(0.2),
                usage_rate_lower_limit: Some(0.0),
            }],
        }
        .into(),
        frbc::FillLevelTargetProfile {
            message_id: id("m1"),
            start_time: at(),
            elements: vec![frbc::FillLevelTargetProfileElement {
                duration: Duration::from_secs(3600),
                fill_level_range: number_range(),
            }],
        }
        .into(),
        frbc::Instruction {
            message_id: id("m1"),
            id: id("instr0"),
            actuator_id: id("actuator1"),
            operation_mode: id("om0"),
            operation_mode_factor: 1.0,
            execution_time: at(),
            abnormal_condition: false,
        }
        .into(),
        // --- DDBC ---
        ddbc::SystemDescription {
            message_id: id("m1"),
            valid_from: at(),
            actuators: vec![ddbc_actuator()],
            present_demand_rate: None,
            provides_average_demand_rate_forecast: true,
        }
        .into(),
        ddbc::ActuatorStatus {
            message_id: id("m1"),
            actuator_id: id("actuator1"),
            active_operation_mode_id: id("om0"),
            operation_mode_factor: 0.5,
            previous_operation_mode_id: Some(id("om1")),
            transition_timestamp: Some(at()),
        }
        .into(),
        ddbc::TimerStatus {
            message_id: id("m1"),
            timer_id: id("timer0"),
            actuator_id: id("actuator1"),
            finished_at: at(),
        }
        .into(),
        ddbc::AverageDemandRateForecast {
            message_id: id("m1"),
            start_time: at(),
            elements: vec![ddbc::AverageDemandRateForecastElement {
                duration: Duration::from_secs(900),
                demand_rate_upper_limit: Some(1.0),
                demand_rate_upper_95ppr: Some(0.9),
                demand_rate_upper_68ppr: Some(0.8),
                demand_rate_expected: 0.5,
                demand_rate_lower_68ppr: Some(0.4),
                demand_rate_lower_95ppr: Some(0.2),
                demand_rate_lower_limit: Some(0.0),
            }],
        }
        .into(),
        ddbc::PresentDemandStatus {
            message_id: id("m1"),
            present_demand_rate: NumberRange::new(0.0, 6000.0),
        }
        .into(),
        ddbc::Instruction {
            message_id: id("m1"),
            id: id("instr0"),
            execution_time: at(),
            abnormal_condition: false,
            actuator_id: id("actuator1"),
            operation_mode_id: id("om0"),
            operation_mode_factor: 1.0,
        }
        .into(),
    ]
}
