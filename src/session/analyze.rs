//! Watching a conversation without being in it.
//!
//! *Is this conversation legal?* is a question neither
//! [`Context::empty`](crate::validate::Context::empty) nor a session can answer. An empty
//! context knows nothing of what came before; a session knows only its own half — a
//! Resource Manager's registry holds what it *received*, so replaying a transcript
//! through one validates the instructions and misses the descriptions they name.
//!
//! An observer is the third thing: one [`Registry`] fed from **both** directions, and
//! every message judged against it.
//!
//! ```
//! use s2_kit::prelude::*;
//! use s2_kit::session::Analyzer;
//! use s2_kit::types::common::EnergyManagementRole::{Cem, Rm};
//! use s2_kit::validate::rules;
//!
//! let now = Timestamp::UNIX_EPOCH;
//! let mut analyzer = Analyzer::new();
//!
//! // The manager instructs an actuator the resource never described.
//! let observed = analyzer.observe(
//!     Cem,
//!     r#"{"message_type":"FRBC.Instruction","message_id":"m1","id":"i1",
//!         "actuator_id":"nope","operation_mode":"om1","operation_mode_factor":0.5,
//!         "execution_time":"1970-01-01T00:00:00Z","abnormal_condition":false}"#,
//!     now,
//! );
//! // Nothing but a stateful observer can say this: the message is schema-perfect.
//! assert!(observed.report.contains(rules::NOT_ALLOWED_IN_STATE));
//! let _ = Rm;
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use super::core::Registry;
use crate::codec::{self, DecodeError, DecodeOptions, Strictness};
use crate::message::{Message, MessageKind};
use crate::types::common::{ControlType, EnergyManagementRole, ReceptionStatusValues};
use crate::types::{Duration, Id, Timestamp, WireProfile};
use crate::validate::{Report, Validate, rules};

/// What an [`Analyzer`] made of one frame.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Observed {
    /// Which side sent it.
    pub sender: EnergyManagementRole,
    /// What it claimed to be, where that could be read at all.
    pub kind: Option<MessageKind>,
    /// The message, when it decoded.
    pub message: Option<Message>,
    /// Properties the schema does not define, removed in [`Strictness::Lenient`].
    pub pruned: Vec<String>,
    /// Everything wrong with it — the intra-message rules and the cross-message ones.
    pub report: Report,
    /// The `ReceptionStatus` a **receiver** should have answered this message with.
    ///
    /// The same choice [`RmSession`](super::RmSession) and
    /// [`CemSession`](super::CemSession) make inside `handle_text`, by the same code, so a
    /// recorded conversation can be diffed against it.
    ///
    /// `TEMPORARY_ERROR` and `PERMANENT_ERROR` are never produced: they are about the
    /// receiver rather than the message, and depend on an
    /// [`InboundPolicy`](super::InboundPolicy) an observer does not have.
    pub status: ReceptionStatusValues,
}

impl Observed {
    /// Whether anything at all was reported.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.report.is_empty()
    }

    /// Whether a receiver would have accepted this message.
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.status == ReceptionStatusValues::Ok
    }

    /// The `diagnostic_label` a refusal should carry: the first error, rule id first.
    #[must_use]
    pub fn diagnostic_label(&self) -> Option<String> {
        self.report.diagnostic_label()
    }
}

/// A stateful observer of a conversation it is not part of.
///
/// Feed it every frame, in order, with the role that sent it. It keeps one [`Registry`]
/// from both sides, tracks the active control type, and validates each message with the
/// full cross-message [`Context`](crate::validate::Context) — so `S2-FRBC-003`,
/// `S2-STATE-001`, `S2-INST-001` and `S2-PEBC-006` fire here and nowhere else outside a
/// live session.
///
/// Decoding is **lenient** by default: an analyzer that refuses a message carrying a
/// vendor extension hides the peer's real bug behind its own strictness.
/// [`Analyzer::strict`] is the other choice.
#[derive(Debug, Clone)]
pub struct Analyzer {
    registry: Registry,
    profile: WireProfile,
    strictness: Strictness,
    active: Option<ControlType>,
    seen: Vec<Id>,
    skew_tolerance: Duration,
    s2_connect: bool,
    max_message_bytes: usize,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    /// An observer that has seen nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Registry::default(),
            profile: WireProfile::V1_0_0,
            strictness: Strictness::Lenient,
            active: None,
            seen: Vec::new(),
            skew_tolerance: Duration::from_secs(30),
            s2_connect: false,
            max_message_bytes: DecodeOptions::DEFAULT_MAX_BYTES,
        }
    }

    /// Decode against a particular wire profile.
    ///
    /// A transcript of a `0.0.2-beta` session must be read as one, or every
    /// `DDBC.SystemDescription` in it is a profile mismatch.
    #[must_use]
    pub const fn profile(mut self, profile: WireProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Refuse a message with a property the schema does not define, as an endpoint would.
    #[must_use]
    pub const fn strict(mut self) -> Self {
        self.strictness = Strictness::Strict;
        self
    }

    /// Read the conversation as one carried by S2 Connect, where a handshake is redundant.
    #[must_use]
    pub const fn over_s2_connect(mut self) -> Self {
        self.s2_connect = true;
        self
    }

    /// How far a peer's clock may differ before it is worth reporting.
    #[must_use]
    pub const fn skew_tolerance(mut self, tolerance: Duration) -> Self {
        self.skew_tolerance = tolerance;
        self
    }

    /// Everything the conversation has published, from both sides.
    #[must_use]
    pub const fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The control type in force, as the conversation has selected it.
    #[must_use]
    pub const fn active_control_type(&self) -> Option<ControlType> {
        self.active
    }

    /// Observe one frame.
    ///
    /// Total: a frame that is not JSON, not S2 or not in the profile yields a report
    /// carrying `S2-MSG-004`, not an error — an analyzer that stops at the first bad frame
    /// stops exactly where it was needed.
    pub fn observe(
        &mut self,
        sender: EnergyManagementRole,
        text: &str,
        now: Timestamp,
    ) -> Observed {
        let options = DecodeOptions {
            strictness: self.strictness,
            profile: self.profile,
            max_bytes: self.max_message_bytes,
        };
        // A description whose `valid_from` has arrived is in force before this message is
        // judged against it.
        let _ = self.registry.activate_due(now);

        match codec::decode_with(text, &options) {
            Err(error) => Self::on_decode_error(sender, &error),
            Ok(decoded) => {
                let mut report = Report::new();
                for path in &decoded.pruned {
                    report.push(
                        rules::UNKNOWN_PROPERTY,
                        path,
                        "property is not defined by the schema and was removed",
                    );
                }
                let kind = decoded.message.kind();
                {
                    let ctx = self.registry.context(
                        self.profile,
                        sender,
                        self.active,
                        self.s2_connect,
                        now,
                        &self.seen,
                        self.skew_tolerance,
                    );
                    report.extend(decoded.message.validate(&ctx));
                }
                let status = report.reception_status();
                self.record(&decoded.message, now);
                Observed {
                    sender,
                    kind: Some(kind),
                    message: Some(decoded.message),
                    pruned: decoded.pruned,
                    report,
                    status,
                }
            }
        }
    }

    fn on_decode_error(sender: EnergyManagementRole, error: &DecodeError) -> Observed {
        let mut report = Report::new();
        report.push(
            rules::DECODE_FAILED,
            "",
            alloc::string::ToString::to_string(error),
        );
        Observed {
            sender,
            kind: error.peek().and_then(|p| p.kind),
            message: None,
            pruned: Vec::new(),
            report,
            status: error.status_value(),
        }
    }

    /// Fold a message into what is known, whichever direction it went.
    fn record(&mut self, message: &Message, now: Timestamp) {
        match message {
            // A control type is active from the moment it is selected. An observer cannot
            // wait for the acknowledgement the way a `CemSession` does: it may not have
            // been recorded, and a transcript that starts mid-session never carries it.
            Message::SelectControlType(select) => {
                self.active = (select.control_type != ControlType::NoSelection)
                    .then_some(select.control_type);
            }
            Message::RevokeObject(revoke) => {
                self.registry.revoke(revoke.object_type, revoke.object_id);
            }
            _ => self.registry.record(message, Some(now)),
        }
        if let Some(id) = message.id()
            && !self.seen.contains(&id)
        {
            self.seen.push(id);
            if self.seen.len() > Registry::MAX_TRACKED {
                self.seen.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::EnergyManagementRole::{Cem, Rm};
    // Explicit because the crate is `no_std`: without `std` there is no prelude to take
    // `.to_string()` from, and `cargo test --lib --no-default-features` is a CI job.
    use alloc::string::ToString;
    use alloc::vec;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap_or(Timestamp::UNIX_EPOCH)
    }

    const DETAILS: &str = r#"{"message_type":"ResourceManagerDetails","message_id":"d1",
        "resource_id":"battery-1","roles":[{"role":"ENERGY_STORAGE","commodity":"ELECTRICITY"}],
        "instruction_processing_delay":500,
        "available_control_types":["FILL_RATE_BASED_CONTROL"],
        "provides_forecast":false,
        "provides_power_measurement_types":["ELECTRIC.POWER.3_PHASE_SYMMETRIC"]}"#;

    const SYSTEM: &str = r#"{"message_type":"FRBC.SystemDescription","message_id":"s1",
        "valid_from":"2024-01-01T00:00:00Z",
        "actuators":[{"id":"actuator1","supported_commodities":["ELECTRICITY"],
          "operation_modes":[{"id":"om1","elements":[{"fill_level_range":{"start_of_range":0,"end_of_range":100},
            "fill_rate":{"start_of_range":0,"end_of_range":0.001},
            "power_ranges":[{"start_of_range":0,"end_of_range":5000,
              "commodity_quantity":"ELECTRIC.POWER.3_PHASE_SYMMETRIC"}]}],
            "abnormal_condition_only":false}],
          "transitions":[],"timers":[]}],
        "storage":{"provides_leakage_behaviour":false,"provides_fill_level_target_profile":false,
          "provides_usage_forecast":false,
          "fill_level_range":{"start_of_range":0,"end_of_range":100}}}"#;

    fn instruction(id: &str, actuator: &str) -> alloc::string::String {
        alloc::format!(
            r#"{{"message_type":"FRBC.Instruction","message_id":"{id}","id":"i-{id}",
                "actuator_id":"{actuator}","operation_mode":"om1","operation_mode_factor":0.5,
                "execution_time":"2024-01-01T12:00:00Z","abnormal_condition":false}}"#
        )
    }

    /// Drive a legal opening: details, control type selected, system description.
    fn opened() -> Analyzer {
        let mut a = Analyzer::new();
        let now = at("2024-01-01T12:00:00Z");
        assert!(a.observe(Rm, DETAILS, now).is_clean());
        assert!(
            a.observe(
                Cem,
                r#"{"message_type":"SelectControlType","message_id":"c1",
                    "control_type":"FILL_RATE_BASED_CONTROL"}"#,
                now
            )
            .is_clean()
        );
        assert_eq!(
            a.active_control_type(),
            Some(ControlType::FillRateBasedControl)
        );
        let observed = a.observe(Rm, SYSTEM, now);
        assert!(observed.is_clean(), "{}", observed.report);
        a
    }

    #[test]
    fn an_instruction_is_checked_against_a_description_the_observer_only_watched() {
        // The whole point: neither side of this exchange is the observer, and no single
        // session holds both halves. The `FRBC.SystemDescription` came from the resource
        // and the instruction from the manager.
        let mut a = opened();
        let now = at("2024-01-01T12:00:01Z");

        let good = a.observe(Cem, &instruction("m1", "actuator1"), now);
        assert!(good.is_clean(), "{}", good.report);

        let bad = a.observe(Cem, &instruction("m2", "nope"), now);
        assert!(
            bad.report.contains(rules::FRBC_UNKNOWN_ACTUATOR),
            "{}",
            bad.report
        );
    }

    #[test]
    fn the_state_table_applies_to_a_conversation_nobody_is_in() {
        let mut a = Analyzer::new();
        let now = at("2024-01-01T12:00:00Z");
        // No control type has been selected, so an instruction is out of order.
        let observed = a.observe(Cem, &instruction("m1", "actuator1"), now);
        assert!(observed.report.contains(rules::NOT_ALLOWED_IN_STATE));

        // And a message from the wrong side is caught too.
        let observed = a.observe(
            Rm,
            r#"{"message_type":"SelectControlType","message_id":"c1",
            "control_type":"FILL_RATE_BASED_CONTROL"}"#,
            now,
        );
        assert!(observed.report.contains(rules::NOT_ALLOWED_FOR_ROLE));
    }

    #[test]
    fn a_reused_instruction_identifier_is_seen_across_the_whole_conversation() {
        let mut a = opened();
        let now = at("2024-01-01T12:00:01Z");
        let first = a.observe(Cem, &instruction("m1", "actuator1"), now);
        assert!(first.is_clean(), "{}", first.report);
        // Same instruction id, different message id.
        let text = instruction("m1", "actuator1").replace("\"m1\"", "\"m2\"");
        let again = a.observe(Cem, &text, now);
        assert!(
            again.report.contains(rules::DUPLICATE_INSTRUCTION_ID),
            "{}",
            again.report
        );
    }

    #[test]
    fn a_vendor_extension_is_reported_rather_than_refused() {
        let mut a = Analyzer::new();
        let now = at("2024-01-01T12:00:00Z");
        let observed = a.observe(
            Rm,
            r#"{"message_type":"FRBC.StorageStatus","message_id":"m1",
                "present_fill_level":52.0,"acme_cell_temperature":31.4}"#,
            now,
        );
        assert_eq!(observed.pruned, vec!["/acme_cell_temperature".to_string()]);
        assert!(observed.report.contains(rules::UNKNOWN_PROPERTY));
        assert!(observed.message.is_some(), "the message is still forwarded");

        // Strictly, the same frame does not decode at all — and that is a finding, not a
        // reason to stop reading the file.
        let mut a = Analyzer::new().strict();
        let observed = a.observe(
            Rm,
            r#"{"message_type":"FRBC.StorageStatus","message_id":"m1",
                "present_fill_level":52.0,"acme_cell_temperature":31.4}"#,
            now,
        );
        assert!(observed.message.is_none());
        assert!(observed.report.contains(rules::DECODE_FAILED));
        assert_eq!(observed.kind, Some(MessageKind::FrbcStorageStatus));
    }

    #[test]
    fn a_frame_that_is_not_s2_at_all_is_a_finding_and_not_the_end() {
        let mut a = Analyzer::new();
        let now = at("2024-01-01T12:00:00Z");
        let observed = a.observe(Rm, "{ not json", now);
        assert!(observed.report.contains(rules::DECODE_FAILED));
        assert_eq!(observed.kind, None);
        // And the next frame is still read.
        assert!(a.observe(Rm, DETAILS, now).is_clean());
    }

    #[test]
    fn a_description_published_for_later_is_in_force_when_its_time_comes() {
        let mut a = Analyzer::new();
        let early = at("2024-01-01T12:00:00Z");
        a.observe(
            Cem,
            r#"{"message_type":"SelectControlType","message_id":"c1",
                "control_type":"FILL_RATE_BASED_CONTROL"}"#,
            early,
        );
        let later = SYSTEM.replace("2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z");
        assert!(a.observe(Rm, &later, early).is_clean());
        // Queued, not in force: nothing yet describes an actuator, so an instruction is
        // unjudgeable rather than wrong — a rule that fired here would accuse the CEM of
        // naming an actuator that simply has not been announced yet.
        assert!(a.registry().frbc.is_none());
        let too_soon = a.observe(Cem, &instruction("m1", "actuator1"), early);
        assert!(!too_soon.report.contains(rules::FRBC_UNKNOWN_ACTUATOR));

        // After `valid_from` it is in force, and now the same instruction is checked
        // against it — including the one that names something it does not describe.
        let after = at("2024-01-02T00:00:01Z");
        let now_fine = a.observe(Cem, &instruction("m2", "actuator1"), after);
        assert!(now_fine.is_clean(), "{}", now_fine.report);
        assert!(a.registry().frbc.is_some());
        let wrong = a.observe(Cem, &instruction("m3", "nope"), after);
        assert!(wrong.report.contains(rules::FRBC_UNKNOWN_ACTUATOR));
    }
}
