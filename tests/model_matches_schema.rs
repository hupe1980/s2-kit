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
use s2_kit::testing::every_message;

fn id(s: &str) -> Id {
    Id::parse(s).expect("a valid identifier")
}

fn at() -> Timestamp {
    Timestamp::from_unix(1_724_508_922, 0)
}
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
        actuators: vec![match every_message().into_iter().find_map(|m| match m {
            Message::DdbcSystemDescription(d) => d.actuators.first().cloned(),
            _ => None,
        }) {
            Some(actuator) => actuator,
            None => panic!("the corpus has a DDBC.SystemDescription"),
        }],
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
                    schema::Kind::Scalar
                    | schema::Kind::Id
                    | schema::Kind::ScalarArray(_)
                    | schema::Kind::IdArray(_) => {}
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
