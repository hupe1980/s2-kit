//! The shape of S2 JSON, as data.
//!
//! [`TYPES`] is generated from the official schema files by
//! `cargo xtask gen-schema-table`: every object type, its properties in schema order,
//! which of them are required, and the bounds on every array. Two things use it.
//!
//! * **Lenient decoding.** A proxy or analyzer has to forward a message it would refuse
//!   to send, so [`prune_unknown`] strips properties the schema does not define and
//!   reports them, instead of failing the whole message.
//! * **Cardinality rules.** `maxItems` is on more than forty arrays; transcribing those
//!   numbers into the validator by hand would be forty chances to get one wrong.
//!
//! The full schema files are vendored under `schema/s2-json/` and used by the tests,
//! which check the hand-written Rust types against them in both directions. They are not
//! embedded in the library: a table of names and bounds is a few kilobytes, and the
//! schemas are three hundred.

mod table;

use alloc::string::String;
use alloc::vec::Vec;

pub use table::{BETA_TYPES, TYPES};

use crate::types::WireProfile;

/// The `minItems` and `maxItems` an array property carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayBounds {
    /// The fewest elements the schema allows.
    pub min: Option<u64>,
    /// The most elements the schema allows.
    pub max: Option<u64>,
}

impl ArrayBounds {
    /// Whether `len` satisfies both bounds.
    #[must_use]
    pub const fn accepts(self, len: u64) -> bool {
        let above = match self.min {
            Some(m) => len >= m,
            None => true,
        };
        let below = match self.max {
            Some(m) => len <= m,
            None => true,
        };
        above && below
    }
}

/// What a property holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A number, string, boolean or enumeration.
    Scalar,
    /// An S2 [`Id`](crate::types::Id).
    ///
    /// A scalar as far as the decoder is concerned, and distinguished here because a tool
    /// that walks a message often wants to *follow* identifiers: to rewrite them, to link
    /// them, to see which of them a message introduces. Telling them apart by property
    /// name would be a hand-maintained list of forty-odd spellings, which is the thing
    /// this table exists to avoid.
    Id,
    /// Another object type, named here so a walk can recurse into it.
    Object(&'static str),
    /// An array of scalars.
    ScalarArray(ArrayBounds),
    /// An array of [`Id`](crate::types::Id)s.
    IdArray(ArrayBounds),
    /// An array of another object type.
    ObjectArray(&'static str, ArrayBounds),
}

impl Kind {
    /// The array bounds, if this property is an array.
    #[must_use]
    pub const fn bounds(self) -> Option<ArrayBounds> {
        match self {
            Kind::ScalarArray(b) | Kind::IdArray(b) | Kind::ObjectArray(_, b) => Some(b),
            Kind::Scalar | Kind::Id | Kind::Object(_) => None,
        }
    }

    /// Whether this property holds one or more S2 identifiers.
    #[must_use]
    pub const fn is_id(self) -> bool {
        matches!(self, Kind::Id | Kind::IdArray(_))
    }
}

/// One property of an S2 object type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Property {
    /// The name as it appears on the wire, misspellings included.
    pub name: &'static str,
    /// Whether the schema lists it in `required`.
    pub required: bool,
    /// What it holds.
    pub kind: Kind,
}

/// One S2 object type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeSpec {
    /// The schema file's name: `FRBC.Instruction`, `NumberRange`.
    pub name: &'static str,
    /// The `message_type` constant, for the 36 types that are messages.
    pub message_type: Option<&'static str>,
    /// Its properties, in schema order.
    pub properties: &'static [Property],
}

impl TypeSpec {
    /// The named property, if the type has one.
    #[must_use]
    pub fn property(&self, name: &str) -> Option<&'static Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

/// The specification for a type, by schema name.
#[must_use]
pub fn type_spec(name: &str) -> Option<&'static TypeSpec> {
    TYPES
        .binary_search_by(|t| t.name.cmp(name))
        .ok()
        .and_then(|i| TYPES.get(i))
}

/// The specification for a type as a given wire profile defines it.
///
/// The two tagged versions of S2 JSON differ in exactly one type, so this is an overlay
/// with a fall-through rather than two tables: [`BETA_TYPES`] first when the profile is
/// `v0.0.2-beta`, [`TYPES`] otherwise and as the fallback.
///
/// Using this rather than [`type_spec`] is what keeps a lenient decoder from pruning
/// `DDBC.SystemDescription.present_demand_rate` — a **required** property of that
/// profile, and absent from `v1.0.0` — out of a message and then refusing it for the
/// field it just removed.
#[must_use]
pub fn type_spec_in(profile: WireProfile, name: &str) -> Option<&'static TypeSpec> {
    match profile {
        WireProfile::V0_0_2Beta => BETA_TYPES
            .iter()
            .find(|t| t.name == name)
            .or_else(|| type_spec(name)),
        WireProfile::V1_0_0 => type_spec(name),
    }
}

/// The specification for a message, by its `message_type`.
#[must_use]
pub fn message_spec(message_type: &str) -> Option<&'static TypeSpec> {
    TYPES.iter().find(|t| t.message_type == Some(message_type))
}

/// Every message type the vendored schemas define.
pub fn message_types() -> impl Iterator<Item = &'static str> {
    TYPES.iter().filter_map(|t| t.message_type)
}

/// How deep [`prune_unknown`] will walk.
///
/// The schema's own deepest path is
/// `PPBC.PowerProfileDefinition → container → sequence → element → value`, five levels.
/// Twice that is room for a schema that grows and a bound that does not depend on a value
/// a peer chose: the walk is driven by the *schema*, but the value decides how many array
/// elements each level has, and a recursion whose depth nothing states is a recursion
/// nobody has checked.
pub const MAX_PRUNE_DEPTH: usize = 16;

/// Removes properties the schema does not define, recursively, reporting each.
///
/// This is what `Strictness::Lenient` does before decoding. It is deliberately *not* a
/// validator: it removes what the types cannot represent and leaves everything else —
/// including values that are out of range — for [`crate::validate`] to judge and report.
///
/// Returns the number of properties removed. Each is named in `removed` as a JSON
/// pointer, so a proxy can tell its operator exactly what it dropped. Below
/// [`MAX_PRUNE_DEPTH`] nothing is pruned and nothing is reported: the typed decode that
/// follows refuses the message anyway, so the only thing a deeper walk buys is a deeper
/// stack.
pub fn prune_unknown(
    value: &mut serde_json::Value,
    type_name: &str,
    profile: WireProfile,
    removed: &mut Vec<String>,
) -> usize {
    prune_at(value, type_name, profile, "", removed, MAX_PRUNE_DEPTH)
}

fn prune_at(
    value: &mut serde_json::Value,
    type_name: &str,
    profile: WireProfile,
    path: &str,
    removed: &mut Vec<String>,
    depth: usize,
) -> usize {
    let Some(depth) = depth.checked_sub(1) else {
        return 0;
    };
    let Some(spec) = type_spec_in(profile, type_name) else {
        return 0;
    };
    let Some(object) = value.as_object_mut() else {
        return 0;
    };

    let mut count = 0;
    let unknown: Vec<String> = object
        .keys()
        .filter(|k| k.as_str() != "message_type" && spec.property(k).is_none())
        .cloned()
        .collect();
    for key in unknown {
        removed.push(alloc::format!("{path}/{key}"));
        object.remove(&key);
        count += 1;
    }

    for property in spec.properties {
        let Some(child) = object.get_mut(property.name) else {
            continue;
        };
        let child_path = alloc::format!("{path}/{}", property.name);
        match property.kind {
            Kind::Object(inner) => {
                count += prune_at(child, inner, profile, &child_path, removed, depth);
            }
            Kind::ObjectArray(inner, _) => {
                if let Some(items) = child.as_array_mut() {
                    for (i, item) in items.iter_mut().enumerate() {
                        let item_path = alloc::format!("{child_path}/{i}");
                        count += prune_at(item, inner, profile, &item_path, removed, depth);
                    }
                }
            }
            Kind::Scalar | Kind::Id | Kind::ScalarArray(_) | Kind::IdArray(_) => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageKind;
    use alloc::string::ToString;

    #[test]
    fn the_table_covers_every_message_the_crate_claims() {
        // The generated table comes from the vendored schema directory; MessageKind is
        // hand-written. If they disagree, one of them is wrong — which is the whole
        // point of generating the table rather than trusting the enum.
        let mut from_schema: Vec<&str> = message_types().collect();
        from_schema.sort_unstable();
        let mut from_rust: Vec<&str> = MessageKind::ALL.iter().map(|k| k.as_str()).collect();
        from_rust.sort_unstable();
        assert_eq!(from_schema, from_rust);
    }

    #[test]
    fn the_beta_overlay_is_the_one_place_the_two_tags_differ() {
        // Every type the overlay names must also exist in the main table, and must differ
        // from it — an overlay entry that is identical is dead weight that hides the one
        // real difference.
        for spec in BETA_TYPES {
            let base = type_spec(spec.name).expect("an overlay type the main table lacks");
            assert_ne!(
                base.properties, spec.properties,
                "{} is in the overlay but identical to v1.0.0",
                spec.name
            );
        }
        // And the difference is a *property*, never an array bound: `check_array` reads
        // the main table alone, which is only sound while that stays true.
        for spec in BETA_TYPES {
            let base = type_spec(spec.name).expect("an overlay type");
            for property in spec.properties {
                if let Some(same) = base.property(property.name) {
                    assert_eq!(
                        same.kind.bounds(),
                        property.kind.bounds(),
                        "{}/{} has different array bounds per profile",
                        spec.name,
                        property.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_property_a_profile_requires_is_not_pruned_out_of_it() {
        // The bug this pins: the property table is v1.0.0's, and
        // `DDBC.SystemDescription.present_demand_rate` is *required* in `0.0.2-beta` and
        // absent from v1.0.0. Pruning a beta message against the v1.0.0 table removed the
        // required field and the typed decode then refused the message for missing it —
        // so an analyzer could not read a conforming beta transcript at all.
        let text = r#"{"message_type":"DDBC.SystemDescription","message_id":"m1",
            "valid_from":"2019-08-24T14:15:22Z","actuators":[],
            "present_demand_rate":{"start_of_range":0.0,"end_of_range":1.0},
            "provides_average_demand_rate_forecast":false,"vendor":1}"#;

        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        let mut removed = Vec::new();
        let n = prune_unknown(
            &mut value,
            "DDBC.SystemDescription",
            WireProfile::V0_0_2Beta,
            &mut removed,
        );
        assert_eq!(n, 1, "only the vendor extension should go");
        assert_eq!(removed, alloc::vec!["/vendor".to_string()]);
        assert!(value.get("present_demand_rate").is_some());

        // And against v1.0.0 it *is* unknown, because that tag removed it.
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        let mut removed = Vec::new();
        prune_unknown(
            &mut value,
            "DDBC.SystemDescription",
            WireProfile::V1_0_0,
            &mut removed,
        );
        assert!(removed.contains(&"/present_demand_rate".to_string()));
    }

    #[test]
    fn types_are_sorted_so_lookup_can_binary_search() {
        for pair in TYPES.windows(2) {
            if let [a, b] = pair {
                assert!(a.name < b.name, "{} should sort before {}", a.name, b.name);
            }
        }
        assert!(type_spec("FRBC.Instruction").is_some());
        assert!(type_spec("NumberRange").is_some());
        assert!(type_spec("Nope").is_none());
    }

    #[test]
    fn array_bounds_came_from_the_schema() {
        let forecast = type_spec("PowerForecast").unwrap();
        let elements = forecast.property("elements").unwrap();
        assert_eq!(
            elements.kind,
            Kind::ObjectArray(
                "PowerForecastElement",
                ArrayBounds {
                    min: Some(1),
                    max: Some(288)
                }
            )
        );
        let details = type_spec("ResourceManagerDetails").unwrap();
        assert_eq!(
            details.property("roles").unwrap().kind.bounds(),
            Some(ArrayBounds {
                min: Some(1),
                max: Some(3)
            })
        );
        // And `PEBC.PowerConstraints` really does demand at least two limit ranges.
        let constraints = type_spec("PEBC.PowerConstraints").unwrap();
        assert_eq!(
            constraints
                .property("allowed_limit_ranges")
                .unwrap()
                .kind
                .bounds()
                .unwrap()
                .min,
            Some(2)
        );
    }

    #[test]
    fn required_flags_came_from_the_schema() {
        let details = type_spec("ResourceManagerDetails").unwrap();
        assert!(details.property("resource_id").unwrap().required);
        assert!(!details.property("name").unwrap().required);
        assert!(!details.property("currency").unwrap().required);
    }

    #[test]
    fn pruning_removes_only_what_the_schema_does_not_define() {
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{
                "message_type": "FRBC.ActuatorStatus",
                "message_id": "m1",
                "actuator_id": "a1",
                "active_operation_mode_id": "om1",
                "operation_mode_factor": 0.5,
                "vendor_extension": {"anything": 1}
            }"#,
        )
        .unwrap();
        let mut removed = Vec::new();
        let n = prune_unknown(
            &mut value,
            "FRBC.ActuatorStatus",
            WireProfile::V1_0_0,
            &mut removed,
        );
        assert_eq!(n, 1);
        assert_eq!(removed, alloc::vec!["/vendor_extension".to_string()]);
        assert!(value.get("vendor_extension").is_none());
        assert!(value.get("operation_mode_factor").is_some());
        assert!(value.get("message_type").is_some(), "the tag must survive");
    }

    #[test]
    fn pruning_stops_at_a_depth_a_peer_did_not_choose() {
        // The schema's deepest real path is five levels, so the cap never fires on a
        // legitimate message — this asserts it exists rather than that it bites.
        const { assert!(MAX_PRUNE_DEPTH > 8) };

        // A nested value as deep as the schema allows is still pruned all the way down.
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{
                "message_type": "PPBC.PowerProfileDefinition",
                "message_id": "m1", "id": "p1",
                "start_time": "2024-01-01T00:00:00Z",
                "end_time": "2024-01-02T00:00:00Z",
                "power_sequences_containers": [
                  {"id": "c1", "power_sequences": [
                    {"id": "s1", "elements": [
                      {"duration": 900000, "power_values": [
                        {"value_expected": 1.0,
                         "commodity_quantity": "ELECTRIC.POWER.L1",
                         "surprise": 1}]}],
                     "is_interruptible": false, "abnormal_condition_only": false}]}]
            }"#,
        )
        .unwrap();
        let mut removed = Vec::new();
        assert_eq!(
            prune_unknown(
                &mut value,
                "PPBC.PowerProfileDefinition",
                WireProfile::V1_0_0,
                &mut removed
            ),
            1
        );
        assert_eq!(
            removed,
            alloc::vec![
                "/power_sequences_containers/0/power_sequences/0/elements/0/power_values/0/surprise"
                    .to_string()
            ]
        );
    }

    #[test]
    fn pruning_reaches_into_nested_objects_and_arrays() {
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{
                "message_type": "FRBC.SystemDescription",
                "message_id": "m1",
                "valid_from": "2019-08-24T14:15:22Z",
                "actuators": [
                  {"id": "a1", "supported_commodities": ["ELECTRICITY"],
                   "operation_modes": [
                     {"id": "om1", "elements": [], "abnormal_condition_only": false,
                      "surprise": 1}
                   ],
                   "transitions": [], "timers": [], "mystery": true}
                ],
                "storage": {"provides_leakage_behaviour": false,
                            "provides_fill_level_target_profile": false,
                            "provides_usage_forecast": false,
                            "fill_level_range": {"start_of_range": 0, "end_of_range": 100,
                                                 "hidden": 2}}
            }"#,
        )
        .unwrap();
        let mut removed = Vec::new();
        let n = prune_unknown(
            &mut value,
            "FRBC.SystemDescription",
            WireProfile::V1_0_0,
            &mut removed,
        );
        assert_eq!(n, 3, "{removed:?}");
        removed.sort();
        assert_eq!(
            removed,
            alloc::vec![
                "/actuators/0/mystery".to_string(),
                "/actuators/0/operation_modes/0/surprise".to_string(),
                "/storage/fill_level_range/hidden".to_string(),
            ]
        );
    }
}
