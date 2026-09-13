//! The rule catalogue.
//!
//! One table, from which `cargo xtask render-rules` writes the published catalogue.
//! Every rule
//! quotes the sentence it implements, so a disagreement with another implementation can
//! be settled by reading rather than by arguing.
//!
//! **Severity is a judgement about the specification, not about the message.** A rule is
//! an `Error` when the standard states the requirement plainly, and a `Warning` when the
//! text is ambiguous, silent, or contradicts itself — with the issue number or the `E`
//! number in the citation. That is what keeps this crate
//! from refusing traffic that another conforming implementation is entitled to send.

use super::{RuleId, Severity};

/// One entry of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// Its stable identifier, which travels in `ReceptionStatus.diagnostic_label`.
    pub id: RuleId,
    /// Whether breaking it makes a message invalid or merely suspect.
    pub severity: Severity,
    /// What the rule checks.
    pub summary: &'static str,
    /// Where it comes from, quoted where the standard states it.
    pub source: &'static str,
    /// Whether the rule needs more than the message in front of it.
    pub needs_context: bool,
}

macro_rules! rules {
    ($(
        $konst:ident = $id:literal, $severity:ident, $context:literal, $summary:literal, $source:literal;
    )*) => {
        $(
            #[doc = $summary]
            ///
            #[doc = $source]
            pub const $konst: RuleId = RuleId($id);
        )*

        /// Every rule, in identifier order.
        pub static RULES: &[Rule] = &[$(
            Rule {
                id: RuleId($id),
                severity: Severity::$severity,
                summary: $summary,
                source: $source,
                needs_context: $context,
            },
        )*];
    };
}

rules![
    // --- identifiers and numbers -----------------------------------------
    DUPLICATE_ID = "S2-ID-001", Error, false,
        "Identifiers must be unique within the scope the standard names",
        "`S2J`: \"Must be unique in the scope of the Resource Manager, for at least the duration of the session\", and per actuator for FRBC and DDBC operation modes.";
    NON_FINITE = "S2-NUM-001", Error, false,
        "Every number must be finite",
        "JSON has no spelling for NaN or infinity, so a non-finite value cannot be sent at all.";
    RANGE_ORDER = "S2-NUM-002", Error, false,
        "A range's start must not be after its end",
        "`S2J schemas/PEBC.AllowedLimitRange.range_boundary`: \"The start of the range shall be smaller or equal than the end of the range.\"";
    RANGE_STRICT_ORDER = "S2-NUM-003", Error, false,
        "A fill-level band's start must be strictly below its end",
        "`S2J schemas/FRBC.OperationModeElement.fill_level_range`: \"The start of the NumberRange shall be smaller than the end of the NumberRange.\"";
    FACTOR_RANGE = "S2-NUM-004", Error, false,
        "An operation-mode factor must be within [0, 1]",
        "`S2J`: \"The factor should be greater than or equal to 0 and less or equal to 1.\"";

    // --- message shape ----------------------------------------------------
    ARRAY_BOUNDS = "S2-MSG-001", Error, false,
        "An array must respect the schema's minItems and maxItems",
        "Generated from `schema/s2-json`; the bounds are the schema's own.";
    DUPLICATE_MESSAGE_ID = "S2-MSG-002", Warning, true,
        "A message_id should not repeat within a session",
        "Implied rather than stated: nothing in S2 JSON requires uniqueness, but a repeated id makes reception statuses ambiguous. A warning for that reason.";
    TIMESTAMP_SKEW = "S2-MSG-003", Warning, true,
        "A timestamp is further from the local clock than its meaning allows",
        "No rule in the standard; a peer whose clock is wrong by hours will plan against the wrong quarter hour. Two thresholds, because S2 timestamps mean two things: one that says when something *happened* (`measurement_timestamp`, `transition_timestamp`, `InstructionStatusUpdate.timestamp`) must not be in the future by more than the configured skew tolerance, while one that says when something *will* happen (`valid_from`, `start_time`, `execution_time`) is expected to be in the future and is only remarked on beyond a year. Never an error: two conforming implementations with different clocks must still be able to talk, and nothing is ever reported for a timestamp in the past, which the standard says means \"already\".";


    // The three below are raised by the session engines rather than by `Validate`: they
    // describe how a message *arrived*, which no amount of looking at a decoded message
    // can tell you. They are rules all the same, because the point of a rule identifier
    // is that a `diagnostic_label` is greppable across a fleet — and "the frame did not
    // decode", "the frame carried a property the schema does not define" and "this
    // device cannot act on it right now" are three different things an operator needs to
    // be able to count separately.
    DECODE_FAILED = "S2-MSG-004", Error, false,
        "The frame could not be decoded into an S2 message",
        "`S2J schemas/ReceptionStatusValues`: INVALID_DATA is \"Message not understood (e.g. not valid JSON, no message_id found)\" and INVALID_MESSAGE is \"Message was not according to schema\". Which of the two is chosen is `DecodeError::status_value`; this identifier is what carries it into the diagnostic label.";
    UNKNOWN_PROPERTY = "S2-MSG-005", Warning, false,
        "A property the schema does not define was removed before decoding",
        "`S2J` sets `additionalProperties: false` on every object, so a strict endpoint refuses such a message outright. A proxy reading in `Strictness::Lenient` forwards it instead and says what it dropped — a warning, because the proxy is not the party entitled to refuse.";
    APPLICATION_REFUSED = "S2-MSG-006", Error, false,
        "The receiving application refused the message through its inbound policy",
        "`S2J schemas/ReceptionStatusValues`: TEMPORARY_ERROR is \"Receiver encountered an error ... try to send the message again\" and PERMANENT_ERROR \"an error which it cannot recover from\". Neither is a judgement about the message, so no other rule can express it; it is the one status a `Validate` implementation could never choose.";

    ROLE_REQUIRED_FIELD = "S2-MSG-007", Error, false,
        "A field that is optional in general is mandatory for this sender's role",
        "`S2J messages/Handshake.supported_protocol_versions`: \"This field is mandatory for the RM, but optional for the CEM.\" The schema cannot express a requirement that depends on another field's value, so it lists the property as optional and says the rest in prose.";

    PROFILE_FIELD = "S2-MSG-008", Error, true,
        "A field must be present exactly when the negotiated wire profile defines it",
        "`S2C §Versioning of JSON Schema files`: the negotiated version selects which schema the messages are read against, and the two tagged versions of S2 JSON differ in one field — `DDBC.SystemDescription.present_demand_rate` is required in `v0.0.2-beta` and was removed in `v1.0.0`, where `DDBC.PresentDemandStatus` carries it instead (erratum E13). The decoder already refuses the wrong shape on the way in; this is the same rule applied on the way *out*, so an endpoint learns at the call site rather than from the peer's `INVALID_MESSAGE`.";

    // --- measurements and forecasts --------------------------------------
    ONE_VALUE_PER_QUANTITY = "S2-PM-001", Error, false,
        "At most one value per CommodityQuantity",
        "`S2J messages/PowerMeasurement.values`: \"at most one item per 'commodity_quantity'\", and the same for every PowerRange and PowerForecastValue array.";
    PHASE_EXCLUSIVITY = "S2-PM-002", Warning, false,
        "Per-phase and three-phase-symmetric quantities should not be mixed",
        "Undocumented; `[s2-json #25]` asks for the exclusivity to be written down (erratum E12). A warning until it is.";
    PERCENTILE_ORDER = "S2-PM-003", Warning, false,
        "Confidence bands should be ordered from the lower limit to the upper",
        "Inferred from the percentile semantics: a 68 % band lies inside a 95 % band. Not stated, so a warning.";
    MEASUREMENT_NOT_OFFERED = "S2-PM-004", Error, true,
        "A measurement may only use a quantity the resource said it provides",
        "`S2J messages/ResourceManagerDetails.provides_power_measurement_types`: \"Array of all CommodityQuantities that this Resource Manager can provide measurements for.\"";
    FORECAST_NOT_OFFERED = "S2-PM-005", Error, true,
        "A forecast may only be sent by a resource that said it provides forecasts",
        "`S2J messages/ResourceManagerDetails.provides_forecast`: \"Indicates whether the ResourceManager is able to provide PowerForecasts.\"";

    // --- resource manager details ----------------------------------------
    CONTROL_TYPE_NOT_OFFERED = "S2-RMD-001", Error, true,
        "The CEM may only select a control type the resource offers",
        "`S2J messages/SelectControlType.control_type`: \"Must be one of the available ControlTypes as defined in the ResourceManagerDetails.\"";
    CURRENCY_REQUIRED = "S2-RMD-002", Warning, true,
        "A resource that publishes costs should declare a currency",
        "`S2J messages/ResourceManagerDetails.currency`: \"Mandatory if cost information is published.\" Not stated is what counts as publishing it: the standard\'s own heat-pump walkthrough sends running_costs and transition_costs of zero while saying \"This heat pump does not define any costs related parameters, so no currency needs to be provided\" (erratum E17). Refusing that example would make this crate stricter than the documentation it implements, so it is a warning.";
    DUPLICATE_ROLE = "S2-RMD-003", Warning, false,
        "A resource should declare each role once per commodity",
        "`S2J messages/ResourceManagerDetails.roles` is \"one or more energy Roles\" bounded at `maxItems: 3` \u{2014} exactly the number of `RoleType` values, while `Commodity` has four. A cap of three cannot be counting commodities, so it is counting role types, and a battery that is storage, load *and* generator for electricity is saying so in the only way the schema allows (erratum E30). What this rule reports is therefore the narrow case the schema still cannot express: the same `(role, commodity)` pair twice, which carries no information either way. A warning rather than an error, for the same reason as `S2-ACT-002` \u{2014} the schema does not say the items are distinct.";
    NO_SELECTION_OFFERED = "S2-RMD-004", Warning, false,
        "NO_SELECTION is not something a resource can offer",
        "`S2J schemas/ControlType`: NO_SELECTION is \"to be used if no control type is or has been selected\" — a state, not a capability.";

    // --- session state ----------------------------------------------------
    NOT_ALLOWED_IN_STATE = "S2-STATE-001", Error, true,
        "The message is not allowed in the session's current state",
        "`S2C §State of communication`, the normative table of what may be sent when.";
    NOT_ALLOWED_FOR_ROLE = "S2-STATE-002", Error, true,
        "The message is not allowed from this role",
        "`S2C §State of communication`: the table has a column per role.";
    HANDSHAKE_UNDER_CONNECT = "S2-STATE-003", Warning, true,
        "Handshake messages are redundant under S2 Connect",
        "`S2C §Communication - JSON messages`: \"the Handshake and HandshakeResponse messages can not be sent. They are redundant by the pairing and session initiation process.\"";

    // --- actuator descriptions --------------------------------------------
    // Both FRBC and DDBC describe actuators with a `supported_commodities` list, so
    // these are one pair of rules rather than two.
    COMMODITY_WITHOUT_POWER_RANGE = "S2-ACT-001", Warning, false,
        "Every operation mode should publish a power range for each commodity the actuator supports",
        "The schema does not require it, but `s2-python` — the reference implementation — refuses an FRBC.ActuatorDescription whose operation mode elements do not cover every entry of `supported_commodities`, so a message without them will not reach a peer built on it. A warning rather than an error because this crate refuses only what the standard refuses; the point of the rule is that the gap is worth seeing before a field trial finds it.";
    DUPLICATE_SUPPORTED_COMMODITY = "S2-ACT-002", Warning, false,
        "An actuator should list each supported commodity once",
        "The schema bounds the array at four but does not say the items are distinct. `s2-python` rejects a repeat; a duplicate carries no information either way, so this crate reports rather than refuses.";

    // --- operation mode statuses ------------------------------------------
    PREVIOUS_MODE_MISSING = "S2-STATUS-001", Warning, true,
        "A status after the first for the same actuator should name the previous mode",
        "`S2J messages/FRBC.ActuatorStatus.previous_operation_mode_id`: \"This value shall always be provided, unless the active FRBC.OperationMode is the first FRBC.OperationMode the Resource Manager is aware of.\" OMBC.Status and DDBC.ActuatorStatus carry the same field with the same sentence, so this is one rule rather than three — and \"the first the Resource Manager is aware of\" is per actuator, not per session.";
    UNKNOWN_TIMER = "S2-STATUS-002", Error, true,
        "A timer status must name a timer the description declares",
        "`S2J schemas/Timer.id`: an identifier is \"unique in the scope of the OMBC.SystemDescription, FRBC.ActuatorDescription or DDBC.ActuatorDescription in which it is used\", so a `*.TimerStatus` naming a timer no description declares refers to nothing. Its own identifier rather than the transition rule's, because a `diagnostic_label` is worth having only if one identifier means one condition (D28) — and \"this transition points at a timer that does not exist\" and \"you are reporting on a timer that does not exist\" are two conditions an operator counts separately.";

    // --- instructions -----------------------------------------------------
    DUPLICATE_INSTRUCTION_ID = "S2-INST-001", Error, true,
        "An instruction identifier must not be reused within a session",
        "`S2J`: \"Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM.\"";
    UNKNOWN_INSTRUCTION = "S2-INST-002", Error, true,
        "An InstructionStatusUpdate must name an instruction that was sent",
        "`S2J messages/InstructionStatusUpdate.instruction_id`: \"ID of this instruction (as provided by the CEM)\".";
    STATUS_REGRESSION = "S2-INST-003", Warning, true,
        "An instruction's status should not move backwards",
        "The order statuses may take is undocumented (`[s2-json #26]`, erratum E11), so going from SUCCEEDED back to STARTED is a warning rather than an error.";
    ABNORMAL_ONLY = "S2-INST-004", Error, true,
        "An abnormal-condition-only mode, transition or range needs abnormal_condition",
        "`S2J`: \"Indicates if this ... may only be used during an abnormal condition.\"";
    BLOCKED_BY_TIMER = "S2-INST-005", Warning, true,
        "The instructed transition is blocked by a timer that has not finished",
        "`S2J schemas/Transition.blocking_timers`: \"List of IDs of Timers that block this Transition from initiating while at least one of these Timers is not yet finished.\" A warning because the CEM's view of a timer is always slightly stale.";
    NO_SUCH_TRANSITION = "S2-INST-006", Warning, true,
        "No transition is described from the active operation mode to the instructed one",
        "the S2 documentation, *Operation modes*: \"a RM can activate another Operation Mode without the CEM requesting it, and it is even allowed to activate an Operation Mode even if there is no transition\" — so this is advice, not a refusal.";

    // --- revocation -------------------------------------------------------
    REVOKE_ROLE_MISMATCH = "S2-REV-001", Warning, true,
        "The revoked object type is not one this sender owns",
        "`S2C §State of communication` lists RevokeObject only for the RM, while `S2J schemas/RevokableObjects` contains every instruction type, which only a CEM sends (erratum E6).";
    REVOKE_UNKNOWN = "S2-REV-002", Warning, true,
        "The revoked object was never published on this session",
        "Not stated. Revoking something the peer never saw is harmless but almost always a bug.";

    // --- FRBC -------------------------------------------------------------
    FRBC_ELEMENTS_CONTIGUOUS = "S2-FRBC-001", Error, false,
        "An operation mode's fill-level bands must be contiguous",
        "`S2J schemas/FRBC.OperationMode.elements`: \"The fill_level_ranges of the items in the Array must be contiguous.\"";
    FRBC_LEAKAGE_CONTIGUOUS = "S2-FRBC-002", Error, false,
        "Leakage bands must be contiguous",
        "`S2J messages/FRBC.LeakageBehaviour.elements`: \"The fill_level_ranges of the elements must be contiguous.\"";
    FRBC_UNKNOWN_ACTUATOR = "S2-FRBC-003", Error, true,
        "An instruction must name an actuator the system description declares",
        "`S2J messages/FRBC.Instruction.actuator_id`: \"ID of the actuator this instruction belongs to.\"";
    FRBC_UNKNOWN_MODE = "S2-FRBC-004", Error, true,
        "An instruction must name an operation mode that actuator has",
        "`S2J messages/FRBC.Instruction.operation_mode`: \"ID of the FRBC.OperationMode that should be activated.\"";
    FRBC_TRANSITION_REFERENCES = "S2-FRBC-005", Error, false,
        "A transition must name operation modes and timers of its own actuator",
        "`S2J schemas/Transition`: the ids are \"unique in the scope of the ... FRBC.ActuatorDescription in which it is used\".";
    FRBC_NOT_OFFERED = "S2-FRBC-006", Error, true,
        "Leakage, usage forecast and target profile may only be sent if the storage offers them",
        "`S2J schemas/FRBC.StorageDescription`: the three `provides_*` flags.";
    FRBC_FILL_LEVEL_OUT_OF_RANGE = "S2-FRBC-007", Warning, true,
        "The reported fill level is outside the range the storage described",
        "`S2J schemas/FRBC.StorageDescription.fill_level_range`: \"When the fill_level is not within this range, the Resource Manager can ignore instructions from the CEM.\" Reported so a CEM knows why it is being ignored.";

    // --- PEBC -------------------------------------------------------------
    PEBC_BOTH_LIMITS = "S2-PEBC-001", Error, false,
        "Power constraints need at least one upper and one lower allowed range",
        "`S2J messages/PEBC.PowerConstraints.allowed_limit_ranges`: \"There shall be at least one PEBC.AllowedLimitRange for the UPPER_LIMIT and at least one AllowedLimitRange for the LOWER_LIMIT.\"";
    PEBC_VALIDITY_WINDOW = "S2-PEBC-002", Error, false,
        "valid_from must precede valid_until",
        "`S2J messages/PEBC.PowerConstraints`: a window that ends before it starts is never in force.";
    PEBC_ENVELOPE_LIMIT_ORDER = "S2-PEBC-003", Error, false,
        "An envelope element's lower limit must not exceed its upper limit",
        "`S2J schemas/PEBC.PowerEnvelopeElement.upper_limit`: \"The lower_limit must be smaller or equal to the upper_limit.\"";
    PEBC_ONE_ENVELOPE_PER_QUANTITY = "S2-PEBC-004", Error, false,
        "At most one envelope per CommodityQuantity",
        "`S2J messages/PEBC.Instruction.power_envelopes`: \"at most one PEBC.PowerEnvelope for each CommodityQuantity.\"";
    PEBC_UNKNOWN_CONSTRAINTS = "S2-PEBC-005", Error, true,
        "An instruction must reference power constraints that are published and in force",
        "`S2J messages/PEBC.Instruction.power_constraints_id`: \"Identifier of the PEBC.PowerConstraints this PEBC.Instruction was based on.\"";
    PEBC_OUTSIDE_ALLOWED = "S2-PEBC-006", Error, true,
        "An envelope limit must lie within an allowed range for its quantity",
        "`S2J schemas/PEBC.PowerEnvelopeElement.upper_limit`: \"shall be in accordance with the constraints provided by the Resource Manager through any PEBC.AllowedLimitRange with limit_type UPPER_LIMIT.\"";
    PEBC_ENERGY_ORDER = "S2-PEBC-007", Error, false,
        "The lower average power must not exceed the upper",
        "`S2J messages/PEBC.EnergyConstraint`: the description of `lower_average_power` repeats the line above it by mistake and says \"greater than or equal to lower_average_power\"; it means at most `upper_average_power` (erratum E3).";

    // --- PPBC -------------------------------------------------------------
    PPBC_WINDOW = "S2-PPBC-001", Error, false,
        "A profile's start must not be after its end",
        "`S2J messages/PPBC.PowerProfileDefinition`: `start_time` is \"the first possible time the first PPBC.PowerSequence could start\" and `end_time` when the last \"shall be finished at the latest\".";
    PPBC_UNKNOWN_SEQUENCE = "S2-PPBC-002", Error, true,
        "An instruction must resolve profile, container and sequence together",
        "`S2J messages/PPBC.ScheduleInstruction`: the three ids name a profile, a container within it, and a sequence within that.";
    PPBC_NOT_INTERRUPTIBLE = "S2-PPBC-003", Error, true,
        "Only a sequence that says it is interruptible may be interrupted",
        "`S2J schemas/PPBC.PowerSequence.is_interruptible`: \"Indicates whether the option of pausing a sequence is available.\"";
    PPBC_STATUS_COVERAGE = "S2-PPBC-004", Error, true,
        "A profile status must cover every container of the profile",
        "`S2J messages/PPBC.PowerProfileStatus.sequence_container_status`: \"Array with status information for all PPBC.PowerSequenceContainers in the PPBC.PowerProfileDefinition.\"";
    PPBC_PROGRESS = "S2-PPBC-005", Error, false,
        "A status for a sequence that has started must report its progress",
        "`S2J schemas/PPBC.PowerSequenceContainerStatus.progress`: \"A value must be provided, unless no sequence has been selected or the selected sequence hasn't started yet.\" This is the half the sentence states plainly: once the selected sequence is running, the CEM is told how far in it is.";
    PPBC_WINDOW_TOO_SHORT = "S2-PPBC-006", Warning, false,
        "The window is shorter than the shortest sequence offered",
        "Implied by the two together: a task that cannot finish inside its own window is one no CEM can schedule.";
    PPBC_PROGRESS_UNEXPECTED = "S2-PPBC-007", Warning, false,
        "Progress was reported for a sequence that has not started",
        "The converse of `S2-PPBC-005`, and it is **not stated**. `S2J schemas/PPBC.PowerSequenceContainerStatus.progress` says a value \"must be provided, unless no sequence has been selected or the selected sequence hasn\u{2019}t started yet\" — which grants permission to omit it, and does not forbid sending it. A `progress` of zero beside a `SCHEDULED` status is redundant rather than wrong, and refusing it would make this crate stricter than the standard (D4). A warning, so a peer that means something by it is still heard.";
    PPBC_SEQUENCE_NOT_NAMED = "S2-PPBC-008", Error, false,
        "A status that says a sequence was selected must name which one",
        "`S2J schemas/PPBC.PowerSequenceContainerStatus.selected_sequence_id`: \"When no ID is given, no sequence was selected yet.\" Read the other way round, which is the way a receiver reads it: a status whose `status` is anything but NOT_SCHEDULED asserts that a sequence *was* selected, and a CEM that is not told which one cannot match the progress to a duration.";

    // --- OMBC -------------------------------------------------------------
    OMBC_UNKNOWN_MODE = "S2-OMBC-001", Error, true,
        "An instruction must name an operation mode the system description declares",
        "`S2J messages/OMBC.Instruction.operation_mode_id`: \"ID of the OMBC.OperationMode that should be activated.\"";
    OMBC_TRANSITION_REFERENCES = "S2-OMBC-002", Error, false,
        "A transition must name operation modes and timers of its own description",
        "`S2J schemas/Transition`: the ids are \"unique in the scope of the OMBC.SystemDescription ... in which it is used\".";

    // --- DDBC -------------------------------------------------------------
    DDBC_UNKNOWN_ACTUATOR = "S2-DDBC-001", Error, true,
        "An instruction must name an actuator the system description declares",
        "`S2J messages/DDBC.Instruction.actuator_id`: \"ID of the actuator this Instruction belongs to.\"";
    DDBC_UNKNOWN_MODE = "S2-DDBC-002", Error, true,
        "An instruction must name an operation mode that actuator has",
        "`S2J messages/DDBC.Instruction.operation_mode_id`: \"ID of the DDBC.OperationMode\".";
    DDBC_TRANSITION_REFERENCES = "S2-DDBC-003", Error, false,
        "A transition must name operation modes and timers of its own actuator",
        "`S2J schemas/Transition`: the ids are \"unique in the scope of the ... DDBC.ActuatorDescription in which it is used\".";
    DDBC_FORECAST_NOT_OFFERED = "S2-DDBC-004", Error, true,
        "A demand forecast may only be sent if the system description offers one",
        "`S2J messages/DDBC.SystemDescription.provides_average_demand_rate_forecast`: \"Indicates whether the Resource Manager could provide a demand rate forecast.\"";
];

impl Rule {
    /// The rule with this identifier.
    #[must_use]
    pub fn find(id: RuleId) -> Option<&'static Rule> {
        RULES.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn identifiers_are_unique_and_well_formed() {
        let mut ids: Vec<&str> = RULES.iter().map(|r| r.id.as_str()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate rule identifier");

        for rule in RULES {
            let id = rule.id.as_str();
            let mut parts = id.split('-');
            assert_eq!(parts.next(), Some("S2"), "{id}");
            let area = parts.next().unwrap_or_default();
            assert!(
                !area.is_empty() && area.chars().all(char::is_uppercase),
                "{id}"
            );
            let number = parts.next().unwrap_or_default();
            assert_eq!(number.len(), 3, "{id} should end in three digits");
            assert!(number.chars().all(|c| c.is_ascii_digit()), "{id}");
            assert_eq!(parts.next(), None, "{id}");
        }
    }

    #[test]
    fn the_table_really_is_in_identifier_order() {
        // The doc comment on `RULES` says so, the rendered catalogue inherits the order,
        // and a reader looking for `S2-MSG-005` should find it between 004 and 006.
        let mut previous: Option<(&str, u32)> = None;
        for rule in RULES {
            let id = rule.id.as_str();
            let mut parts = id.split('-').skip(1);
            let area = parts.next().unwrap_or_default();
            let number: u32 = parts.next().unwrap_or_default().parse().unwrap_or(0);
            if let Some((previous_area, previous_number)) = previous
                && previous_area == area
            {
                assert!(
                    number > previous_number,
                    "{id} follows S2-{previous_area}-{previous_number:03}"
                );
            }
            previous = Some((area, number));
        }
    }

    #[test]
    fn every_rule_cites_a_source_and_says_what_it_checks() {
        for rule in RULES {
            assert!(rule.summary.len() > 20, "{} has no summary", rule.id);
            assert!(rule.source.len() > 30, "{} has no citation", rule.id);
        }
    }

    #[test]
    fn ambiguity_is_a_warning_and_a_plain_requirement_is_an_error() {
        // The rules that exist because the standard is unclear must not refuse traffic.
        for rule in RULES {
            let cites_ambiguity = rule.source.contains("Not stated")
                || rule.source.contains("not stated")
                || rule.source.contains("Undocumented")
                || rule.source.contains("undocumented")
                || rule.source.contains("Inferred")
                || rule.source.contains("Implied");
            if cites_ambiguity {
                assert_eq!(
                    rule.severity,
                    Severity::Warning,
                    "{} rests on an ambiguity and must not be an error",
                    rule.id
                );
            }
        }
    }

    #[test]
    fn lookup_works() {
        assert_eq!(Rule::find(FRBC_UNKNOWN_MODE).unwrap().id, FRBC_UNKNOWN_MODE);
        assert!(Rule::find(RuleId("S2-NOPE-001")).is_none());
    }
}
