//! The S2 JSON data model: 36 messages and 41 component types.
//!
//! Types common to every control type are in [`common`]; the rest are grouped by the
//! control type they belong to ([`pebc`], [`ppbc`], [`ombc`], [`frbc`], [`ddbc`]). The
//! scalar types the whole model rests on — [`Id`], [`Timestamp`], [`Duration`] — are
//! re-exported here.
//!
//! The model is **wire-faithful**, warts included: `DDBC.OperationMode` really does
//! capitalise its `Id` field, `DDBC.ActuatorDescription` really does spell it
//! `supported_commodites`, and `ControlType` really does say `NOT_CONTROLABLE`. Each is
//! carried by a `#[serde(rename)]` so the bytes are right while the Rust identifier is
//! not also wrong, and each is listed as errata.
//!
//! What the types do **not** do is judge. A factor of 1.3 and a 289-element forecast are
//! both representable, because a proxy has to be able to carry a message it would refuse
//! to send, and because `INVALID_CONTENT` is a reception status the standard wants sent —
//! which requires having decoded the message first. [`crate::validate`] is what judges;
//! the builders are what refuse.

pub mod common;
pub mod ddbc;
pub mod frbc;
pub mod id;
pub mod ombc;
pub mod pebc;
pub mod ppbc;
pub mod time;

pub use common::*;
pub use id::{Id, InvalidId};
pub use time::{Duration, InvalidTimestamp, TimeOutOfRange, Timestamp};

use alloc::borrow::ToOwned;
use alloc::string::String;
use serde::{Deserialize, Serialize};

/// An S2 JSON version string as it appears on the wire.
///
/// S2 negotiates versions by **exact string match**: `S2C §Versioning of JSON Schema
/// files` says "the exact version string **must** be used (e.g. `v1.0.0`)". No semver
/// range is involved, and none should be inferred — `s2energy` parses the peer's string
/// as a `VersionReq`, which accepts strings the peer never offered.
///
/// The complication is that the deployed ecosystem negotiates `"0.0.2-beta"` — the tag
/// that predates S2 JSON v1.0.0 — while the schemas are tagged `v1.0.0`. Both are
/// supported; [`WireProfile`] is what a version string means for the wire.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolVersion(String);

impl ProtocolVersion {
    /// S2 JSON v1.0.0, the current tag.
    pub const V1_0_0: &'static str = "1.0.0";
    /// S2 JSON v0.0.2-beta, what every deployed implementation still negotiates.
    pub const V0_0_2_BETA: &'static str = "0.0.2-beta";

    /// Wrap a version string exactly as it will appear on the wire.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The version without the optional `v` the two specifications disagree about.
    ///
    /// `S2C` requires `v1.0.0`, every S2 JSON handshake writes `1.0.0`, and
    /// `s2energy-connection`'s examples write `v1`. This crate transmits one spelling and
    /// understands both (erratum E25).
    #[must_use]
    pub fn canonical(&self) -> &str {
        self.0.strip_prefix('v').unwrap_or(&self.0)
    }

    /// Whether two version strings name the same version.
    ///
    /// Exact match on the canonical form — **not** a semver range, which is how
    /// `s2energy` accepts versions the peer never offered (D5). The leading `v` is the
    /// only latitude.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.canonical() == other.canonical()
    }

    /// Which wire shape this version string selects, if this crate knows it.
    #[must_use]
    pub fn wire_profile(&self) -> Option<WireProfile> {
        match self.canonical() {
            "1.0.0" => Some(WireProfile::V1_0_0),
            "0.0.2-beta" => Some(WireProfile::V0_0_2Beta),
            _ => None,
        }
    }

    /// Whether this crate can speak this version.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.wire_profile().is_some()
    }
}

impl From<&str> for ProtocolVersion {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for ProtocolVersion {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl core::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::fmt::Debug for ProtocolVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ProtocolVersion({:?})", self.0)
    }
}

/// A shape of the S2 JSON wire.
///
/// The two tagged versions of S2 JSON differ in exactly one place. Content-diffing them
/// (ignoring `$id`) shows:
///
/// | | `v0.0.2-beta` | `v1.0.0` |
/// |---|---|---|
/// | `DDBC.SystemDescription` | required `present_demand_rate` | field absent |
/// | `DDBC.PresentDemandStatus` | does not exist | a message |
/// | everything else | identical | identical |
///
/// So one enum and one `Option` carry the whole difference, and the codec enforces it
/// per negotiated version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum WireProfile {
    /// S2 JSON v1.0.0.
    #[default]
    V1_0_0,
    /// S2 JSON v0.0.2-beta — what the deployed ecosystem still negotiates.
    V0_0_2Beta,
}

impl WireProfile {
    /// The version string this crate offers for the profile **in an S2 JSON handshake**.
    ///
    /// No `v`, which is what every deployed implementation negotiates;
    /// [`connect_version_str`](Self::connect_version_str) is the other spelling.
    #[must_use]
    pub const fn version_str(self) -> &'static str {
        match self {
            Self::V1_0_0 => ProtocolVersion::V1_0_0,
            Self::V0_0_2Beta => ProtocolVersion::V0_0_2_BETA,
        }
    }

    /// The version string this crate offers for the profile **over S2 Connect**.
    ///
    /// With the `v`, which `S2C §Versioning of JSON Schema files` requires. Only what goes
    /// out: [`ProtocolVersion::matches`] understands either spelling (E25).
    #[must_use]
    pub const fn connect_version_str(self) -> &'static str {
        match self {
            Self::V1_0_0 => "v1.0.0",
            Self::V0_0_2Beta => "v0.0.2-beta",
        }
    }

    /// The version string, as a [`ProtocolVersion`].
    #[must_use]
    pub fn version(self) -> ProtocolVersion {
        ProtocolVersion::new(self.version_str())
    }

    /// The S2 Connect version string, as a [`ProtocolVersion`].
    #[must_use]
    pub fn connect_version(self) -> ProtocolVersion {
        ProtocolVersion::new(self.connect_version_str())
    }

    /// Whether `DDBC.SystemDescription` must carry `present_demand_rate`.
    #[must_use]
    pub const fn ddbc_system_description_carries_demand_rate(self) -> bool {
        matches!(self, Self::V0_0_2Beta)
    }

    /// Every profile this crate speaks, most recent first.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::V1_0_0, Self::V0_0_2Beta]
    }
}

impl core::fmt::Display for WireProfile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.version_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_strings_map_to_profiles_exactly() {
        for (s, expected) in [
            ("1.0.0", Some(WireProfile::V1_0_0)),
            ("v1.0.0", Some(WireProfile::V1_0_0)),
            ("0.0.2-beta", Some(WireProfile::V0_0_2Beta)),
            ("v0.0.2-beta", Some(WireProfile::V0_0_2Beta)),
            // Not a range match: 1.0.1 is not 1.0.0, and we do not guess.
            ("1.0.1", None),
            ("1.1", None),
            ("", None),
            ("latest", None),
        ] {
            assert_eq!(ProtocolVersion::new(s).wire_profile(), expected, "{s:?}");
        }
    }

    #[test]
    fn the_two_specifications_spell_one_version_two_ways_and_both_are_understood() {
        // `S2C §Versioning of JSON Schema files` requires `v1.0.0`; every S2 JSON
        // handshake in the wild writes `1.0.0`. Exact matching on either side means the
        // two do not interoperate, which is erratum E25.
        let connect = ProtocolVersion::new("v1.0.0");
        let handshake = ProtocolVersion::new("1.0.0");
        assert!(connect.matches(&handshake));
        assert!(handshake.matches(&connect));
        assert_eq!(connect.canonical(), "1.0.0");
        assert_eq!(handshake.canonical(), "1.0.0");
        // The string on the wire is untouched: matching is tolerant, transmission is not.
        assert_eq!(connect.as_str(), "v1.0.0");
        assert_eq!(handshake.as_str(), "1.0.0");

        // And each specification gets the spelling it asks for.
        assert_eq!(WireProfile::V1_0_0.version_str(), "1.0.0");
        assert_eq!(WireProfile::V1_0_0.connect_version_str(), "v1.0.0");
        assert_eq!(WireProfile::V0_0_2Beta.version_str(), "0.0.2-beta");
        assert_eq!(WireProfile::V0_0_2Beta.connect_version_str(), "v0.0.2-beta");
        for profile in WireProfile::all() {
            assert!(profile.version().matches(&profile.connect_version()));
            assert_eq!(profile.connect_version().wire_profile(), Some(*profile));
        }
    }

    #[test]
    fn tolerating_the_prefix_is_not_tolerating_a_range() {
        // The one latitude taken is the `v`. Everything else is still an exact match —
        // `s2energy` parsing the peer's string as a `VersionReq` is what makes it accept
        // versions the peer never offered (D5), and `v1` is the API major version, not a
        // schema version at all.
        for s in [
            "1.0",
            "1",
            "v1",
            "1.0.1",
            "^1.0.0",
            "1.0.0-rc1",
            "vv1.0.0",
            "",
        ] {
            assert_eq!(ProtocolVersion::new(s).wire_profile(), None, "{s:?}");
            assert!(
                !ProtocolVersion::new(s).matches(&ProtocolVersion::new("1.0.0")),
                "{s:?} must not match 1.0.0"
            );
        }
    }

    #[test]
    fn only_the_beta_profile_carries_the_ddbc_demand_rate() {
        assert!(WireProfile::V0_0_2Beta.ddbc_system_description_carries_demand_rate());
        assert!(!WireProfile::V1_0_0.ddbc_system_description_carries_demand_rate());
    }
}
