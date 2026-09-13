//! Turning bytes into [`Message`]s, and back.
//!
//! # Two phases, so that every failure can still be answered
//!
//! S2 requires a `ReceptionStatus` for every message received, and the status has to
//! name the message it is about. That is awkward for a message that failed to parse: the
//! `subject_message_id` has to come from somewhere.
//!
//! So decoding happens in two phases. First a [`Peek`] pulls out `message_type`,
//! `message_id` and `subject_message_id`, ignoring everything else and tolerating almost
//! any malformation. Then the typed decode runs, refusing unknown properties. Whatever
//! goes wrong, the error carries what the peek learned, and
//! [`DecodeError::reception_status`] turns it into exactly the status the standard
//! prescribes — `INVALID_DATA` for "not understood", `INVALID_MESSAGE` for "not
//! according to schema" — naming the right message, or the nil identifier when there was
//! nothing to name.
//!
//! ```
//! use s2_kit::codec::{decode, DecodeError};
//!
//! let err = decode("{ not json").unwrap_err();
//! let status = err.reception_status();
//! assert_eq!(status.status, s2_kit::types::common::ReceptionStatusValues::InvalidData);
//! assert_eq!(status.subject_message_id, s2_kit::types::Id::NIL);
//! ```
//!
//! # Bounded before it is parsed
//!
//! `maxItems` does not bound a message meaningfully: a schema-legal
//! `PPBC.PowerProfileDefinition` may hold a thousand containers of 288 sequences of 288
//! elements. [`DecodeOptions::max_bytes`] is checked *before* the parser runs.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::Deserialize;

use crate::message::{Message, MessageKind};
use crate::types::common::{ReceptionStatus, ReceptionStatusValues};
use crate::types::{Id, WireProfile};

/// How much malformation a decoder tolerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strictness {
    /// Refuse anything the schema does not define. What an endpoint should do.
    #[default]
    Strict,
    /// Remove properties the schema does not define, report them, and carry on. What a
    /// proxy or an analyzer should do: forwarding a peer's message with an unknown field
    /// stripped is more useful than refusing to look at it.
    Lenient,
}

/// Everything that shapes a decode.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeOptions {
    /// Strict or lenient.
    pub strictness: Strictness,
    /// The wire profile to decode against.
    pub profile: WireProfile,
    /// The largest message that will be parsed at all.
    pub max_bytes: usize,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            strictness: Strictness::Strict,
            profile: WireProfile::V1_0_0,
            max_bytes: Self::DEFAULT_MAX_BYTES,
        }
    }
}

impl DecodeOptions {
    /// One mebibyte. Large enough for any plausible S2 message, small enough that a peer
    /// cannot exhaust memory with a single legal one.
    pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

    /// Decode against a particular wire profile.
    #[must_use]
    pub const fn profile(mut self, profile: WireProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Strip and report unknown properties instead of refusing the message.
    #[must_use]
    pub const fn lenient(mut self) -> Self {
        self.strictness = Strictness::Lenient;
        self
    }

    /// Change the size cap.
    #[must_use]
    pub const fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = bytes;
        self
    }
}

/// What a decode learned before trying to build a typed message.
///
/// Everything here is optional because everything here can be missing from a message
/// that a peer should not have sent but did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Peek {
    /// The raw `message_type` string, if there was one.
    pub message_type: Option<String>,
    /// The message kind, if this crate knows that type.
    pub kind: Option<MessageKind>,
    /// The `message_id`, if there was one and it was a valid identifier.
    pub message_id: Option<Id>,
    /// The `subject_message_id`, for a `ReceptionStatus`.
    pub subject_message_id: Option<Id>,
}

impl Peek {
    /// The identifier a `ReceptionStatus` about this message should carry.
    ///
    /// The nil identifier when the message had none — which is what the ecosystem does,
    /// and what `[s2-json #21]` is open about.
    #[must_use]
    pub fn subject(&self) -> Id {
        self.message_id
            .or(self.subject_message_id)
            .unwrap_or(Id::NIL)
    }
}

#[derive(Deserialize)]
struct RawPeek<'a> {
    // `Cow`, not `&str`: a peer may write `"FRBC\u002EStorageStatus"`, and a borrowed
    // `&str` cannot hold a string that needed unescaping — serde refuses it, turning a
    // good message into `INVALID_DATA` naming nothing. Unescaped messages still borrow.
    #[serde(borrow, default)]
    message_type: Option<alloc::borrow::Cow<'a, str>>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    subject_message_id: Option<String>,
}

/// Why a message could not be decoded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// Larger than [`DecodeOptions::max_bytes`], and so never parsed.
    #[error("message of {len} bytes exceeds the {max} byte limit")]
    TooLarge {
        /// How big it was.
        len: usize,
        /// How big it was allowed to be.
        max: usize,
    },
    /// Not JSON at all, or not a JSON object.
    #[error("not a JSON object: {reason}")]
    NotJson {
        /// What the parser said.
        reason: String,
    },
    /// No `message_type` property, so there is no way to know what it is.
    #[error("no message_type")]
    NoMessageType {
        /// What the peek found, for the reception status.
        peek: Peek,
    },
    /// A `message_type` this crate does not know.
    #[error("unknown message_type {message_type:?}")]
    UnknownMessageType {
        /// The type that was given.
        message_type: String,
        /// What the peek found.
        peek: Peek,
    },
    /// A message type that does not exist in the negotiated wire profile.
    #[error("{kind} does not exist in S2 JSON {profile}")]
    NotInProfile {
        /// The message.
        kind: MessageKind,
        /// The profile that was being decoded against.
        profile: WireProfile,
        /// What the peek found.
        peek: Peek,
    },
    /// The message does not match the schema: a missing field, a wrong type, an unknown
    /// property in strict mode, or a value outside what the type can hold.
    #[error("{kind} does not match the schema: {reason}")]
    Schema {
        /// Which message it claimed to be.
        kind: MessageKind,
        /// What serde said, which names the offending field.
        reason: String,
        /// What the peek found.
        peek: Peek,
    },
    /// A field the negotiated profile requires is absent, or one it forbids is present.
    #[error("{kind} is not valid in S2 JSON {profile}: {reason}")]
    ProfileMismatch {
        /// Which message.
        kind: MessageKind,
        /// The profile.
        profile: WireProfile,
        /// What is wrong.
        reason: String,
        /// What the peek found.
        peek: Peek,
    },
}

impl DecodeError {
    /// What the peek learned, whatever went wrong afterwards.
    #[must_use]
    pub fn peek(&self) -> Option<&Peek> {
        match self {
            DecodeError::TooLarge { .. } | DecodeError::NotJson { .. } => None,
            DecodeError::NoMessageType { peek }
            | DecodeError::UnknownMessageType { peek, .. }
            | DecodeError::NotInProfile { peek, .. }
            | DecodeError::Schema { peek, .. }
            | DecodeError::ProfileMismatch { peek, .. } => Some(peek),
        }
    }

    /// The reception status the standard prescribes for this failure.
    ///
    /// `S2J schemas/ReceptionStatusValues` draws the line precisely: `INVALID_DATA` is
    /// "Message not understood (e.g. not valid JSON, no message_id found)" and
    /// `INVALID_MESSAGE` is "Message was not according to schema".
    #[must_use]
    pub fn status_value(&self) -> ReceptionStatusValues {
        match self {
            // Not understood at all.
            DecodeError::TooLarge { .. } | DecodeError::NotJson { .. } => {
                ReceptionStatusValues::InvalidData
            }
            DecodeError::NoMessageType { peek } if peek.message_id.is_none() => {
                ReceptionStatusValues::InvalidData
            }
            // Understood enough to answer, but not according to schema.
            _ => ReceptionStatusValues::InvalidMessage,
        }
    }

    /// The complete `ReceptionStatus` to send back.
    #[must_use]
    pub fn reception_status(&self) -> ReceptionStatus {
        ReceptionStatus {
            subject_message_id: self.peek().map_or(Id::NIL, Peek::subject),
            status: self.status_value(),
            diagnostic_label: Some(self.to_string()),
        }
    }
}

/// What a successful decode produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The message.
    pub message: Message,
    /// Properties the schema does not define, removed in [`Strictness::Lenient`].
    ///
    /// Always empty in strict mode, where an unknown property is an error instead.
    pub pruned: Vec<String>,
}

/// Decode one S2 message, strictly, against S2 JSON v1.0.0.
pub fn decode(text: &str) -> Result<Message, DecodeError> {
    decode_with(text, &DecodeOptions::default()).map(|d| d.message)
}

/// Decode one S2 message.
pub fn decode_with(text: &str, options: &DecodeOptions) -> Result<Decoded, DecodeError> {
    if text.len() > options.max_bytes {
        return Err(DecodeError::TooLarge {
            len: text.len(),
            max: options.max_bytes,
        });
    }

    let peek = peek(text)?;

    let Some(message_type) = peek.message_type.clone() else {
        return Err(DecodeError::NoMessageType { peek });
    };
    let Some(kind) = peek.kind else {
        return Err(DecodeError::UnknownMessageType { message_type, peek });
    };
    if !kind.exists_in(options.profile) {
        return Err(DecodeError::NotInProfile {
            kind,
            profile: options.profile,
            peek,
        });
    }

    let (message, pruned) = match options.strictness {
        Strictness::Strict => {
            let message =
                serde_json::from_str::<Message>(text).map_err(|e| DecodeError::Schema {
                    kind,
                    reason: e.to_string(),
                    peek: peek.clone(),
                })?;
            (message, Vec::new())
        }
        Strictness::Lenient => {
            let mut value = serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                DecodeError::NotJson {
                    reason: e.to_string(),
                }
            })?;
            let mut pruned = Vec::new();
            crate::schema::prune_unknown(&mut value, kind.as_str(), &mut pruned);
            let message =
                serde_json::from_value::<Message>(value).map_err(|e| DecodeError::Schema {
                    kind,
                    reason: e.to_string(),
                    peek: peek.clone(),
                })?;
            (message, pruned)
        }
    };

    check_profile(&message, options.profile, &peek)?;
    Ok(Decoded { message, pruned })
}

/// Read the three fields a reception status needs, tolerating everything else.
pub fn peek(text: &str) -> Result<Peek, DecodeError> {
    let raw: RawPeek = serde_json::from_str(text).map_err(|e| DecodeError::NotJson {
        reason: e.to_string(),
    })?;
    Ok(Peek {
        kind: raw.message_type.as_deref().and_then(MessageKind::from_wire),
        message_type: raw.message_type.map(alloc::borrow::Cow::into_owned),
        // An identifier that does not match the pattern is no identifier: a reception
        // status carrying it would be unparseable in its turn.
        message_id: raw.message_id.as_deref().and_then(|s| Id::parse(s).ok()),
        subject_message_id: raw
            .subject_message_id
            .as_deref()
            .and_then(|s| Id::parse(s).ok()),
    })
}

/// The one place the two wire profiles differ.
fn check_profile(message: &Message, profile: WireProfile, peek: &Peek) -> Result<(), DecodeError> {
    if let Message::DdbcSystemDescription(d) = message {
        let needs = profile.ddbc_system_description_carries_demand_rate();
        match (needs, d.present_demand_rate.is_some()) {
            (true, false) => {
                return Err(DecodeError::ProfileMismatch {
                    kind: MessageKind::DdbcSystemDescription,
                    profile,
                    reason: "present_demand_rate is required in v0.0.2-beta".to_owned(),
                    peek: peek.clone(),
                });
            }
            (false, true) => {
                return Err(DecodeError::ProfileMismatch {
                    kind: MessageKind::DdbcSystemDescription,
                    profile,
                    reason: "present_demand_rate was removed in v1.0.0; \
                             send DDBC.PresentDemandStatus instead"
                        .to_owned(),
                    peek: peek.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Encode one S2 message.
///
/// The output is canonical: properties in schema order, absent optionals omitted rather
/// than written as `null`, timestamps in UTC with the fewest fractional digits that lose
/// nothing. Re-encoding a message a proxy did not change is a no-op.
///
/// # Panics
///
/// Never, for any `Message` this crate can construct: the model contains no map with
/// non-string keys and no type whose `Serialize` can fail. A non-finite `f64` would be
/// the one way to reach a failure, and the builders and validator refuse those.
#[must_use]
pub fn encode(message: &Message) -> String {
    serde_json::to_string(message).unwrap_or_else(|e| {
        // Unreachable for a well-formed Message; returning a diagnostic beats panicking
        // in a driver that is trying to answer a peer.
        format!(r#"{{"message_type":"ReceptionStatus","subject_message_id":"00000000-0000-0000-0000-000000000000","status":"PERMANENT_ERROR","diagnostic_label":"s2-kit could not encode a message: {e}"}}"#)
    })
}

/// Encode one S2 message with indentation, for logs and fixtures.
#[must_use]
pub fn encode_pretty(message: &Message) -> String {
    serde_json::to_string_pretty(message).unwrap_or_else(|_| encode(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Timestamp, frbc};

    const STORAGE_STATUS: &str =
        r#"{"message_type":"FRBC.StorageStatus","message_id":"m1","present_fill_level":52.0}"#;

    #[test]
    fn a_good_message_decodes() {
        let m = decode(STORAGE_STATUS).unwrap();
        assert_eq!(m.kind(), MessageKind::FrbcStorageStatus);
        assert_eq!(m.id(), Some(Id::new_const("m1")));
    }

    #[test]
    fn encoding_is_canonical_and_round_trips() {
        let m = decode(STORAGE_STATUS).unwrap();
        assert_eq!(encode(&m), STORAGE_STATUS);
    }

    #[test]
    fn broken_json_is_invalid_data_naming_nothing() {
        let e = decode("{ not json").unwrap_err();
        assert_eq!(e.status_value(), ReceptionStatusValues::InvalidData);
        assert_eq!(e.reception_status().subject_message_id, Id::NIL);
    }

    #[test]
    fn a_schema_violation_is_invalid_message_naming_the_right_id() {
        // Right type, missing a required field.
        let e = decode(r#"{"message_type":"FRBC.StorageStatus","message_id":"m1"}"#).unwrap_err();
        assert_eq!(e.status_value(), ReceptionStatusValues::InvalidMessage);
        let status = e.reception_status();
        assert_eq!(status.subject_message_id, Id::new_const("m1"));
        assert!(
            status
                .diagnostic_label
                .unwrap()
                .contains("present_fill_level")
        );
    }

    #[test]
    fn an_unknown_property_is_refused_in_strict_mode_and_pruned_in_lenient() {
        let text = r#"{"message_type":"FRBC.StorageStatus","message_id":"m1","present_fill_level":52.0,"vendor":1}"#;
        assert!(decode(text).is_err());

        let decoded = decode_with(text, &DecodeOptions::default().lenient()).unwrap();
        assert_eq!(decoded.pruned, alloc::vec!["/vendor".to_string()]);
        assert_eq!(decoded.message.kind(), MessageKind::FrbcStorageStatus);
    }

    #[test]
    fn no_message_type_is_reported_with_whatever_id_was_there() {
        let e = decode(r#"{"message_id":"m1","present_fill_level":1.0}"#).unwrap_err();
        // There was an id, so the peer can be told which message failed; the status is
        // INVALID_MESSAGE rather than INVALID_DATA because we could name it.
        assert_eq!(e.status_value(), ReceptionStatusValues::InvalidMessage);
        assert_eq!(e.reception_status().subject_message_id, Id::new_const("m1"));

        // With nothing to name, it is INVALID_DATA and the nil identifier.
        let e = decode(r#"{"present_fill_level":1.0}"#).unwrap_err();
        assert_eq!(e.status_value(), ReceptionStatusValues::InvalidData);
        assert_eq!(e.reception_status().subject_message_id, Id::NIL);
    }

    #[test]
    fn a_message_type_written_with_json_escapes_still_decodes() {
        // `\u002E` is a dot. JSON says these two documents carry the same string, and a
        // peer is entitled to emit either — some JSON writers escape aggressively. A
        // peek that borrowed the slice would answer "expected a borrowed string" and
        // turn a perfectly good status into INVALID_DATA naming nothing.
        let escaped = "{\"message_type\":\"FRBC\\u002EStorageStatus\",\
                       \"message_id\":\"m1\",\"present_fill_level\":52.0}";
        let peeked = peek(escaped).unwrap();
        assert_eq!(peeked.kind, Some(MessageKind::FrbcStorageStatus));
        assert_eq!(peeked.message_type.as_deref(), Some("FRBC.StorageStatus"));
        assert_eq!(peeked.message_id, Some(Id::new_const("m1")));

        let message = decode(escaped).unwrap();
        assert_eq!(message.kind(), MessageKind::FrbcStorageStatus);
        // And it re-encodes canonically, which is the unescaped spelling.
        assert_eq!(encode(&message), STORAGE_STATUS);
    }

    #[test]
    fn an_unparseable_message_id_is_treated_as_no_id() {
        // `a b` cannot be an S2 identifier, so it cannot be echoed in a reception status.
        let e = decode(r#"{"message_type":"FRBC.StorageStatus","message_id":"a b"}"#).unwrap_err();
        assert_eq!(e.reception_status().subject_message_id, Id::NIL);
    }

    #[test]
    fn an_unknown_message_type_is_named_in_the_diagnostic() {
        let e = decode(r#"{"message_type":"FRBC.Surprise","message_id":"m1"}"#).unwrap_err();
        assert!(matches!(e, DecodeError::UnknownMessageType { .. }));
        assert!(e.to_string().contains("FRBC.Surprise"));
        assert_eq!(e.reception_status().subject_message_id, Id::new_const("m1"));
    }

    #[test]
    fn a_message_larger_than_the_cap_is_never_parsed() {
        let big = alloc::format!(
            r#"{{"message_type":"FRBC.StorageStatus","message_id":"m1","present_fill_level":{}}}"#,
            "1".repeat(2000)
        );
        let options = DecodeOptions::default().max_bytes(512);
        let e = decode_with(&big, &options).unwrap_err();
        assert!(matches!(e, DecodeError::TooLarge { .. }));
        assert_eq!(e.status_value(), ReceptionStatusValues::InvalidData);
    }

    #[test]
    fn the_ddbc_difference_between_the_two_profiles_is_enforced() {
        let beta = r#"{"message_type":"DDBC.SystemDescription","message_id":"m1",
            "valid_from":"2019-08-24T14:15:22Z","actuators":[],
            "present_demand_rate":{"start_of_range":0.0,"end_of_range":1.0},
            "provides_average_demand_rate_forecast":false}"#;
        let one_zero = r#"{"message_type":"DDBC.SystemDescription","message_id":"m1",
            "valid_from":"2019-08-24T14:15:22Z","actuators":[],
            "provides_average_demand_rate_forecast":false}"#;

        // Each is right in its own profile...
        assert!(
            decode_with(
                one_zero,
                &DecodeOptions::default().profile(WireProfile::V1_0_0)
            )
            .is_ok()
        );
        assert!(
            decode_with(
                beta,
                &DecodeOptions::default().profile(WireProfile::V0_0_2Beta)
            )
            .is_ok()
        );
        // ...and wrong in the other.
        let e =
            decode_with(beta, &DecodeOptions::default().profile(WireProfile::V1_0_0)).unwrap_err();
        assert!(matches!(e, DecodeError::ProfileMismatch { .. }), "{e}");
        let e = decode_with(
            one_zero,
            &DecodeOptions::default().profile(WireProfile::V0_0_2Beta),
        )
        .unwrap_err();
        assert!(matches!(e, DecodeError::ProfileMismatch { .. }), "{e}");
    }

    #[test]
    fn the_message_new_in_one_zero_is_refused_in_the_beta_profile() {
        let text = r#"{"message_type":"DDBC.PresentDemandStatus","message_id":"m1",
            "present_demand_rate":{"start_of_range":0.0,"end_of_range":1.0}}"#;
        assert!(decode(text).is_ok());
        let e = decode_with(
            text,
            &DecodeOptions::default().profile(WireProfile::V0_0_2Beta),
        )
        .unwrap_err();
        assert!(matches!(e, DecodeError::NotInProfile { .. }), "{e}");
    }

    #[test]
    fn a_reception_status_is_never_answered_but_still_peeks() {
        let text = r#"{"message_type":"ReceptionStatus","subject_message_id":"m1","status":"OK"}"#;
        let p = peek(text).unwrap();
        assert_eq!(p.subject_message_id, Some(Id::new_const("m1")));
        assert_eq!(p.message_id, None);
        assert_eq!(p.subject(), Id::new_const("m1"));
        let m = decode(text).unwrap();
        assert_eq!(m.kind(), MessageKind::ReceptionStatus);
    }

    #[test]
    fn pretty_encoding_still_parses() {
        let m = Message::from(frbc::StorageStatus {
            message_id: Id::new_const("m1"),
            present_fill_level: 52.0,
        });
        let pretty = encode_pretty(&m);
        assert!(pretty.contains('\n'));
        assert_eq!(decode(&pretty).unwrap(), m);
    }

    #[test]
    fn timestamps_survive_a_round_trip_unchanged() {
        let text = r#"{"message_type":"PowerMeasurement","message_id":"m1","measurement_timestamp":"2019-08-24T14:15:22Z","values":[{"commodity_quantity":"ELECTRIC.POWER.L1","value":510.6}]}"#;
        let m = decode(text).unwrap();
        assert_eq!(encode(&m), text);
        let _ = Timestamp::UNIX_EPOCH;
    }
}
