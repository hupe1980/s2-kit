//! [`Id`] — the S2 identifier type.
//!
//! `S2J schemas/ID` defines an identifier as a **string** matching
//! `[a-zA-Z0-9\-_:]{2,64}`. Its description says "An identifier expressed as a UUID",
//! which is not what the pattern says and not what the standard's own examples use —
//! the EV and heat-pump walkthroughs identify actuators as `"actuator1"` and operation
//! modes as `"om1"`. The pattern is the contract (see erratum E1), so
//! `Id` is a validated string and UUID generation is a convenience rather than a
//! requirement.
//!
//! Because the pattern caps an identifier at 64 ASCII characters, `Id` is an inline
//! fixed buffer: [`Copy`], allocation-free and usable without `alloc`. A Resource
//! Manager describes its actuators once and then resolves every instruction against the
//! identifiers it generated, so ids are copied constantly and should not be a field of
//! `.clone()` calls.

use core::fmt;
use core::hash::{Hash, Hasher};
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An S2 identifier: 2 to 64 characters of `[a-zA-Z0-9-_:]`.
///
/// # Examples
///
/// ```
/// use s2_kit::types::Id;
///
/// let from_docs = Id::parse("actuator1").unwrap();
/// assert_eq!(from_docs.as_str(), "actuator1");
///
/// // A compile-time constant, validated during const evaluation.
/// const OFF: Id = Id::new_const("om1");
/// assert_eq!(OFF.as_str(), "om1");
///
/// assert!(Id::parse("a").is_err()); // too short
/// assert!(Id::parse("has space").is_err()); // illegal character
/// ```
#[derive(Clone, Copy)]
pub struct Id {
    bytes: [u8; Id::MAX_LEN],
    len: u8,
}

/// Why a string is not a valid [`Id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidId {
    /// Shorter than [`Id::MIN_LEN`].
    #[error("an S2 identifier must be at least {} characters", Id::MIN_LEN)]
    TooShort,
    /// Longer than [`Id::MAX_LEN`].
    #[error("an S2 identifier must be at most {} characters", Id::MAX_LEN)]
    TooLong,
    /// Contains a character outside `[a-zA-Z0-9-_:]`.
    #[error("an S2 identifier may only contain [a-zA-Z0-9-_:], found {found:?} at byte {index}")]
    IllegalCharacter {
        /// The offending character.
        found: char,
        /// Its byte offset in the input.
        index: usize,
    },
}

impl Id {
    /// The longest identifier the schema pattern allows.
    pub const MAX_LEN: usize = 64;
    /// The shortest identifier the schema pattern allows.
    pub const MIN_LEN: usize = 2;

    /// Whether `b` is one of the characters the pattern allows.
    #[must_use]
    pub const fn is_legal_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
    }

    /// Parse and validate an identifier.
    #[allow(clippy::cast_possible_truncation)] // the length is checked against MAX_LEN first
    pub fn parse(s: &str) -> Result<Self, InvalidId> {
        let raw = s.as_bytes();
        if raw.len() < Self::MIN_LEN {
            return Err(InvalidId::TooShort);
        }
        if raw.len() > Self::MAX_LEN {
            return Err(InvalidId::TooLong);
        }
        for (index, &b) in raw.iter().enumerate() {
            if !Self::is_legal_byte(b) {
                return Err(InvalidId::IllegalCharacter {
                    // Every illegal byte is either ASCII (and so a `char`) or part of a
                    // multi-byte sequence; reporting the replacement character for the
                    // latter is more useful than refusing to name it at all.
                    found: char::from_u32(u32::from(b)).unwrap_or(char::REPLACEMENT_CHARACTER),
                    index,
                });
            }
        }
        let mut bytes = [0u8; Self::MAX_LEN];
        bytes
            .get_mut(..raw.len())
            .unwrap_or(&mut [])
            .copy_from_slice(raw);
        #[allow(clippy::cast_possible_truncation)] // checked against MAX_LEN above
        Ok(Self {
            bytes,
            len: raw.len() as u8,
        })
    }

    /// Build an identifier during const evaluation, for literals in code.
    ///
    /// # Panics
    ///
    /// At compile time, when `s` does not match the S2 identifier pattern.
    #[must_use]
    // const-evaluated: a panic is a compile error, and the length is asserted above.
    #[allow(
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    pub const fn new_const(s: &str) -> Self {
        let raw = s.as_bytes();
        assert!(raw.len() >= Self::MIN_LEN, "S2 identifier is too short");
        assert!(raw.len() <= Self::MAX_LEN, "S2 identifier is too long");
        let mut bytes = [0u8; Self::MAX_LEN];
        let mut i = 0;
        while i < raw.len() {
            assert!(
                Self::is_legal_byte(raw[i]),
                "S2 identifiers may only contain [a-zA-Z0-9-_:]"
            );
            bytes[i] = raw[i];
            i += 1;
        }
        Self {
            bytes,
            len: raw.len() as u8,
        }
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let used = self.bytes.get(..self.len as usize).unwrap_or(&[]);
        // Only bytes that passed `is_legal_byte` are ever stored, and all of them are
        // ASCII, so this cannot fail.
        core::str::from_utf8(used).unwrap_or("")
    }

    /// How many characters the identifier has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Always `false` — an `Id` has at least [`Id::MIN_LEN`] characters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// A fresh random identifier (UUIDv4, hyphenated, lowercase).
    #[cfg(feature = "uuid")]
    #[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
    #[must_use]
    pub fn generate() -> Self {
        Self::from_uuid(uuid::Uuid::new_v4())
    }

    /// A fresh time-ordered identifier (UUIDv7, hyphenated, lowercase).
    ///
    /// Sortable by creation time, which makes transcripts and databases of S2 traffic
    /// cheaper to index than the random v4 form.
    #[cfg(all(feature = "uuid", feature = "std"))]
    #[cfg_attr(docsrs, doc(cfg(all(feature = "uuid", feature = "std"))))]
    #[must_use]
    pub fn generate_v7() -> Self {
        Self::from_uuid(uuid::Uuid::now_v7())
    }

    /// An identifier holding the hyphenated lowercase form of `uuid`.
    #[cfg(feature = "uuid")]
    #[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
    #[must_use]
    pub fn from_uuid(uuid: uuid::Uuid) -> Self {
        let mut buf = [0u8; uuid::fmt::Hyphenated::LENGTH];
        let s = uuid.hyphenated().encode_lower(&mut buf);
        Self::parse(s).unwrap_or(Self {
            bytes: [b'0'; Self::MAX_LEN],
            len: 2,
        })
    }

    /// The identifier as a [`uuid::Uuid`], when it happens to be one.
    ///
    /// Returns `None` for the many legal S2 identifiers that are not UUIDs.
    #[cfg(feature = "uuid")]
    #[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
    #[must_use]
    pub fn as_uuid(&self) -> Option<uuid::Uuid> {
        uuid::Uuid::try_parse(self.as_str()).ok()
    }

    /// The all-zero UUID, which the ecosystem uses as `subject_message_id` when a
    /// message arrived with no readable `message_id` at all.
    ///
    /// `S2J messages/ReceptionStatus` requires the field, and `INVALID_DATA` is defined
    /// as the answer to a message with "no message_id found" — leaving nothing to name.
    /// `s2-python` and `s2energy` both send the nil UUID; `[s2-json #21]` is the open
    /// issue (erratum E10).
    pub const NIL: Self = Self::new_const("00000000-0000-0000-0000-000000000000");
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl core::ops::Deref for Id {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl core::borrow::Borrow<str> for Id {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for Id {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Id {}

impl PartialEq<str> for Id {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Id {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialOrd for Id {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Id {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Id {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:?})", self.as_str())
    }
}

impl FromStr for Id {
    type Err = InvalidId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for Id {
    type Error = InvalidId;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Id;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an S2 identifier matching [a-zA-Z0-9-_:]{2,64}")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Id, E> {
                Id::parse(v).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Id {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "ID".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"[a-zA-Z0-9\-_:]{2,64}",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_identifiers_the_standards_own_examples_use() {
        // From docs `learn/examples/ev` and `learn/examples/heat-pump`. A crate that
        // models `ID` as a UUID rejects every one of these.
        for s in [
            "actuator1",
            "om1",
            "om0",
            "instr0",
            "transition1",
            "timer0",
            "acme_ev_xxxxxx",
            "powerConstraint1",
            "energyconstraint1",
            "pe_xxx",
            "envelope1",
            "a:b",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert_eq!(Id::parse(s).unwrap().as_str(), s, "should accept {s:?}");
        }
    }

    #[test]
    fn rejects_what_the_pattern_rejects() {
        assert_eq!(Id::parse("a"), Err(InvalidId::TooShort));
        assert_eq!(Id::parse(""), Err(InvalidId::TooShort));
        assert_eq!(Id::parse(&"x".repeat(65)), Err(InvalidId::TooLong));
        assert!(matches!(
            Id::parse("a b"),
            Err(InvalidId::IllegalCharacter { index: 1, .. })
        ));
        assert!(matches!(
            Id::parse("a.b"),
            Err(InvalidId::IllegalCharacter { found: '.', .. })
        ));
        // The pattern is unanchored in the schema; we anchor it, so a valid prefix with
        // an invalid tail is rejected rather than silently accepted (E1).
        assert!(Id::parse("valid-then-\u{1F600}").is_err());
    }

    #[test]
    fn longest_legal_identifier_round_trips() {
        let s = "x".repeat(64);
        let id = Id::parse(&s).unwrap();
        assert_eq!(id.as_str(), s);
        assert_eq!(id.len(), 64);
    }

    #[test]
    fn ordering_and_hashing_follow_the_string() {
        let a = Id::parse("aa").unwrap();
        let b = Id::parse("ab").unwrap();
        assert!(a < b);
        assert_eq!(a, Id::parse("aa").unwrap());
        assert_ne!(a, b);
    }

    #[test]
    fn json_is_a_bare_string() {
        let id = Id::parse("actuator1").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"actuator1\"");
        assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
        assert!(serde_json::from_str::<Id>("\"a b\"").is_err());
    }

    #[cfg(feature = "uuid")]
    #[test]
    fn generated_identifiers_are_valid_and_unique() {
        let a = Id::generate();
        let b = Id::generate();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert!(a.as_uuid().is_some());
        assert!(Id::parse("actuator1").unwrap().as_uuid().is_none());
    }

    #[test]
    fn is_copy_and_small() {
        // 65 bytes, no allocation, no destructor: the property the whole design rests on.
        assert_eq!(core::mem::size_of::<Id>(), 65);
        let a = Id::parse("actuator1").unwrap();
        let b = a; // moves nothing
        assert_eq!(a, b);
    }
}
