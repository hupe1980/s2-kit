//! The model is proven against the standard's own JSON schemas, in both directions.
//!
//! This is the test the whole "hand-written, not generated" decision rests on
//!. Three things are checked, each catching a different
//! kind of drift:
//!
//! 1. **Completeness.** One instance of every message, with *every* optional field
//!    populated, is serialised and its key set compared with the schema's property list.
//!    A field this crate forgot entirely is invisible to a round-trip test — the struct
//!    that does not mention it round-trips fixtures that do not mention it either — and
//!    this is what catches it.
//! 2. **Validity.** Every one of those instances is validated against the official
//!    schema by a real JSON Schema validator.
//! 3. **Stability.** Every instance decodes from its own encoding, unchanged.
//!
//! The schemas are vendored under `schema/s2-json/` by `cargo xtask vendor-specs`, at
//! the v1.0.0 tag.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::collections::{BTreeMap, BTreeSet};

use s2_kit::prelude::*;
use s2_kit::schema;
use serde_json::{Map, Value, json};

// ---------------------------------------------------------------------------
// The official schemas, bundled into one document
// ---------------------------------------------------------------------------

/// Loads every vendored schema and rewrites its cross-references into one document, so
/// a validator needs no network and no retriever.
fn bundle() -> Value {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/s2-json");
    let mut defs = Map::new();
    for sub in ["messages", "schemas"] {
        let dir = root.join(sub);
        for entry in std::fs::read_dir(&dir).expect("vendored schemas") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("file name")
                .trim_end_matches(".schema.json")
                .to_string();
            let text = std::fs::read_to_string(&path).expect("readable");
            let mut value: Value = serde_json::from_str(&text).expect("valid JSON");
            strip_and_rewrite(&mut value);
            defs.insert(name, value);
        }
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs,
    })
}

fn strip_and_rewrite(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("$id");
            map.remove("$schema");
            if let Some(Value::String(reference)) = map.get("$ref") {
                let name = reference
                    .rsplit('/')
                    .next()
                    .unwrap_or(reference)
                    .trim_end_matches(".schema.json")
                    .to_string();
                map.insert("$ref".into(), Value::String(format!("#/$defs/{name}")));
            }
            for (_, child) in map.iter_mut() {
                strip_and_rewrite(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_and_rewrite(item);
            }
        }
        _ => {}
    }
}

fn validator_for(bundle: &Value, type_name: &str) -> jsonschema::Validator {
    let mut schema = bundle.clone();
    schema
        .as_object_mut()
        .expect("object")
        .insert("$ref".into(), Value::String(format!("#/$defs/{type_name}")));
    jsonschema::options()
        .build(&schema)
        .expect("the official schema compiles")
}

// ---------------------------------------------------------------------------
// One of everything, with every optional field populated
// ---------------------------------------------------------------------------

fn id(s: &str) -> Id {
    Id::parse(s).expect("a valid identifier")
}

fn at() -> Timestamp {
    "2024-08-24T14:15:22Z".parse().expect("a valid timestamp")
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

/// One of every message, with every optional field set.
fn every_message() -> Vec<Message> {
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

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[test]
fn there_is_one_fully_populated_example_of_every_message() {
    let kinds: BTreeSet<&str> = every_message().iter().map(|m| m.kind().as_str()).collect();
    let expected: BTreeSet<&str> = MessageKind::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(kinds, expected, "every message needs an example here");
}

#[test]
fn every_field_the_schema_defines_exists_in_the_rust_type() {
    // The direction that catches a field we forgot to model at all.
    let mut missing: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();

    for message in every_message() {
        let kind = message.kind();
        let encoded: Value = serde_json::from_str(&encode(&message)).expect("our own output");
        let object = encoded.as_object().expect("a JSON object");
        let ours: BTreeSet<&str> = object
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "message_type")
            .collect();

        let spec = schema::message_spec(kind.as_str()).expect("a vendored schema");
        let theirs: BTreeSet<&str> = spec.properties.iter().map(|p| p.name).collect();

        for name in theirs.difference(&ours) {
            // `DDBC.SystemDescription.present_demand_rate` is the one field that exists
            // only in the older profile, and the v1.0.0 example rightly omits it.
            if kind == MessageKind::DdbcSystemDescription && *name == "present_demand_rate" {
                continue;
            }
            missing.push(format!("{kind}.{name}"));
        }
        for name in ours.difference(&theirs) {
            extra.push(format!("{kind}.{name}"));
        }
    }

    assert!(
        missing.is_empty(),
        "fields the schema has and we do not: {missing:#?}"
    );
    assert!(
        extra.is_empty(),
        "fields we have and the schema does not: {extra:#?}"
    );
}

#[test]
fn required_fields_are_required_and_optional_fields_are_optional() {
    for message in every_message() {
        let kind = message.kind();
        let spec = schema::message_spec(kind.as_str()).expect("a vendored schema");
        let encoded: Value = serde_json::from_str(&encode(&message)).expect("our own output");
        let object = encoded.as_object().expect("a JSON object");

        for property in spec.properties {
            if property.required
                && !(kind == MessageKind::DdbcSystemDescription
                    && property.name == "present_demand_rate")
            {
                assert!(
                    object.contains_key(property.name),
                    "{kind}.{} is required by the schema and absent from our encoding",
                    property.name
                );
            }
        }

        // And no optional field is written as `null`: absent means absent.
        for (name, value) in object {
            assert!(
                !value.is_null(),
                "{kind}.{name} was encoded as null rather than omitted"
            );
        }
    }
}

#[test]
fn everything_we_encode_validates_against_the_official_schema() {
    let bundle = bundle();
    let mut validators: BTreeMap<&str, jsonschema::Validator> = BTreeMap::new();

    for message in every_message() {
        let kind = message.kind();
        let text = encode(&message);
        let instance: Value = serde_json::from_str(&text).expect("our own output");
        let validator = validators
            .entry(kind.as_str())
            .or_insert_with(|| validator_for(&bundle, kind.as_str()));

        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{kind} does not validate: {errors:#?}\n{text}"
        );
    }
}

#[test]
fn everything_the_schema_accepts_from_us_decodes_back_unchanged() {
    for message in every_message() {
        let text = encode(&message);
        let back =
            decode(&text).unwrap_or_else(|e| panic!("{} did not decode: {e}", message.kind()));
        assert_eq!(back, message, "{} did not round-trip", message.kind());
        assert_eq!(encode(&back), text, "{} is not byte-stable", message.kind());
    }
}

#[test]
fn the_beta_profile_carries_the_field_that_moved() {
    // The one wire difference between the two tagged versions, checked against the
    // vendored beta schema rather than against our own belief about it.
    let beta_schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("schema/s2-json/profiles/v0.0.2-beta/DDBC.SystemDescription.schema.json"),
        )
        .expect("the vendored beta schema"),
    )
    .expect("valid JSON");
    let required: Vec<&str> = beta_schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(required.contains(&"present_demand_rate"));

    let message = Message::from(ddbc::SystemDescription {
        message_id: id("m1"),
        valid_from: at(),
        actuators: vec![ddbc_actuator()],
        present_demand_rate: Some(NumberRange::new(0.0, 6000.0)),
        provides_average_demand_rate_forecast: false,
    });
    let text = encode(&message);
    let options = s2_kit::DecodeOptions::default().profile(WireProfile::V0_0_2Beta);
    let decoded = s2_kit::decode_with(&text, &options).expect("valid in the beta profile");
    assert_eq!(decoded.message, message);

    // And the same bytes are refused in v1.0.0, where the field was removed.
    assert!(decode(&text).is_err());
}

#[test]
fn every_component_type_the_schema_defines_is_reachable_from_a_message() {
    // A component nothing references would be one we modelled for nothing — or, worse,
    // one the schema has and no message of ours carries.
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = schema::TYPES
        .iter()
        .filter(|t| t.message_type.is_some())
        .map(|t| t.name)
        .collect();
    while let Some(name) = queue.pop() {
        if !reachable.insert(name) {
            continue;
        }
        if let Some(spec) = schema::type_spec(name) {
            for property in spec.properties {
                match property.kind {
                    schema::Kind::Object(inner) | schema::Kind::ObjectArray(inner, _) => {
                        queue.push(inner);
                    }
                    schema::Kind::Scalar | schema::Kind::ScalarArray(_) => {}
                }
            }
        }
    }

    let all: BTreeSet<&str> = schema::TYPES.iter().map(|t| t.name).collect();
    let orphans: Vec<&&str> = all.difference(&reachable).collect();
    assert!(
        orphans.is_empty(),
        "unreachable component types: {orphans:?}"
    );
}
