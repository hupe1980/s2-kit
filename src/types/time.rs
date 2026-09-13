//! [`Timestamp`] and [`Duration`] — the two time types on the S2 wire.
//!
//! S2 writes instants as RFC 3339 date-times (`S2J` `format: date-time`) and spans as
//! whole milliseconds (`S2J schemas/Duration`: "Duration in milliseconds", minimum 0).
//! Both are modelled here rather than borrowed from a time crate, for three reasons
//!:
//!
//! * a consumer should not have to name a third-party crate in its own manifest, and
//!   keep its major version in step, to call this one — the complaint `hems` filed
//!   against `s2energy`, whose public API is full of `chrono::DateTime<Utc>`;
//! * the core is `no_std + alloc`, and a two-hundred-line parser is smaller than any
//!   time crate;
//! * the wire has exactly two shapes, and neither needs a calendar, a time zone
//!   database or leap-second tables.
//!
//! Conversions to and from [`jiff`](crate::types::time), `time` and `chrono` live behind
//! the features of those names, and each crate is re-exported so a caller can always
//! name the same version this crate was built against.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A span of time in whole milliseconds, as `S2J schemas/Duration` defines it.
///
/// The schema gives `minimum: 0`, so a duration is unsigned — S2 never expresses a
/// negative span.
///
/// ```
/// use s2_kit::types::Duration;
///
/// let d = Duration::from_millis(3_600_000);
/// assert_eq!(d.as_secs_f64(), 3600.0);
/// assert_eq!(serde_json::to_string(&d).unwrap(), "3600000");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Duration(u64);

impl Duration {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// The longest span this type can hold: about 584 million years.
    ///
    /// Useful as "no deadline" in a comparison. Not useful as something to wait for —
    /// converting it to a [`core::time::Duration`] and handing that to a timer is how a
    /// sleep overflows an `Instant`.
    pub const MAX: Self = Self(u64::MAX);

    /// A duration of `millis` milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// A duration of `secs` seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1_000))
    }

    /// The span in milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// The span in seconds, as a float — the unit S2 rates are expressed per.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // exact below 2^53 ms, which is 285 000 years
    pub const fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000.0
    }

    /// Whether the span is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Sum, or `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Difference, or `None` when `other` is longer.
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ms", self.0)
    }
}

impl From<Duration> for core::time::Duration {
    fn from(d: Duration) -> Self {
        Self::from_millis(d.0)
    }
}

impl TryFrom<core::time::Duration> for Duration {
    type Error = TimeOutOfRange;
    fn try_from(d: core::time::Duration) -> Result<Self, Self::Error> {
        u64::try_from(d.as_millis())
            .map(Self)
            .map_err(|_| TimeOutOfRange)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Duration {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "Duration".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "integer", "minimum": 0 })
    }
}

/// A value could not be represented as a [`Timestamp`] or [`Duration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the value is outside the range this time type can represent")]
pub struct TimeOutOfRange;

/// An instant in time, held as seconds and nanoseconds since the Unix epoch, UTC.
///
/// The wire form is RFC 3339. Parsing accepts any RFC 3339 spelling — `Z`, `z`, a
/// numeric offset, `T` or `t` or a space separator, zero to nine fractional digits —
/// and converts to UTC. Formatting always emits `YYYY-MM-DDTHH:MM:SS[.fff]Z` with the
/// fewest of {0, 3, 6, 9} fractional digits that loses nothing, so re-encoding a
/// message a proxy did not change is a no-op.
///
/// Seconds-plus-nanoseconds rather than a single nanosecond count because a peer may
/// legitimately send a far-future instant — `S2J messages/FRBC.TimerStatus.finished_at`
/// explicitly allows "an arbitrary DateTimeStamp in the past", and nothing bounds the
/// future — and a 64-bit nanosecond count only reaches the year 2262.
///
/// ```
/// use s2_kit::types::Timestamp;
///
/// let t: Timestamp = "2019-08-24T14:15:22Z".parse().unwrap();
/// assert_eq!(t.to_string(), "2019-08-24T14:15:22Z");
///
/// // Offsets are normalised to UTC.
/// let t: Timestamp = "2019-08-24T16:15:22+02:00".parse().unwrap();
/// assert_eq!(t.to_string(), "2019-08-24T14:15:22Z");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    secs: i64,
    nanos: u32,
}

/// Why a string is not a valid RFC 3339 date-time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTimestamp {
    /// The overall shape is wrong (too short, missing separator, trailing characters).
    #[error("not an RFC 3339 date-time")]
    Malformed,
    /// A field was out of range (month 13, hour 25, day 31 of February).
    #[error("the {field} field of the date-time is out of range")]
    FieldOutOfRange {
        /// Which field.
        field: &'static str,
    },
    /// The instant cannot be represented.
    #[error("the date-time is outside the representable range")]
    OutOfRange,
}

impl Timestamp {
    /// 1970-01-01T00:00:00Z.
    pub const UNIX_EPOCH: Self = Self { secs: 0, nanos: 0 };

    /// The instant `secs` seconds and `nanos` nanoseconds after the Unix epoch.
    ///
    /// `nanos` is normalised into `0..1_000_000_000`.
    #[must_use]
    pub const fn from_unix(secs: i64, nanos: u32) -> Self {
        let extra = (nanos / 1_000_000_000) as i64;
        Self {
            secs: secs.saturating_add(extra),
            nanos: nanos % 1_000_000_000,
        }
    }

    /// The instant `millis` milliseconds after the Unix epoch.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        let secs = millis.div_euclid(1_000);
        let rem = millis.rem_euclid(1_000);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // rem is 0..999
        Self {
            secs,
            nanos: (rem as u32) * 1_000_000,
        }
    }

    /// Whole seconds since the Unix epoch (floor).
    #[must_use]
    pub const fn unix_secs(self) -> i64 {
        self.secs
    }

    /// The sub-second part, in nanoseconds.
    #[must_use]
    pub const fn subsec_nanos(self) -> u32 {
        self.nanos
    }

    /// Milliseconds since the Unix epoch (floor), or `None` if it does not fit.
    #[must_use]
    pub const fn unix_millis(self) -> Option<i64> {
        match self.secs.checked_mul(1_000) {
            Some(ms) => ms.checked_add((self.nanos / 1_000_000) as i64),
            None => None,
        }
    }

    /// The current instant.
    ///
    /// Provided for drivers, examples and applications. **Nothing in the protocol core
    /// calls it**: every engine takes `now` as a parameter, which is what makes a
    /// two-hour timer a microsecond-long unit test.
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    #[must_use]
    pub fn now() -> Self {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        match d {
            Ok(d) => {
                let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
                Self::from_unix(secs, d.subsec_nanos())
            }
            // Before 1970. Only reachable on a machine whose clock is badly wrong.
            Err(e) => {
                let d = e.duration();
                let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
                Self::from_unix(-secs, 0)
            }
        }
    }

    /// This instant plus `d`, or `None` on overflow.
    #[must_use]
    #[allow(clippy::cast_possible_wrap)] // a Duration of more than i64::MAX ms is 292 million years
    pub const fn checked_add(self, d: Duration) -> Option<Self> {
        let ms = d.as_millis();
        let add_secs = (ms / 1_000) as i64;
        let add_nanos = ((ms % 1_000) as u32) * 1_000_000;
        let mut nanos = self.nanos + add_nanos;
        let mut carry = 0i64;
        if nanos >= 1_000_000_000 {
            nanos -= 1_000_000_000;
            carry = 1;
        }
        match self.secs.checked_add(add_secs) {
            Some(s) => match s.checked_add(carry) {
                Some(s) => Some(Self { secs: s, nanos }),
                None => None,
            },
            None => None,
        }
    }

    /// This instant minus `d`, or `None` on overflow.
    #[must_use]
    #[allow(clippy::cast_possible_wrap)] // as above
    pub const fn checked_sub(self, d: Duration) -> Option<Self> {
        let ms = d.as_millis();
        let sub_secs = (ms / 1_000) as i64;
        let sub_nanos = ((ms % 1_000) as u32) * 1_000_000;
        let (nanos, borrow) = if self.nanos >= sub_nanos {
            (self.nanos - sub_nanos, 0i64)
        } else {
            (self.nanos + 1_000_000_000 - sub_nanos, 1i64)
        };
        match self.secs.checked_sub(sub_secs) {
            Some(s) => match s.checked_sub(borrow) {
                Some(s) => Some(Self { secs: s, nanos }),
                None => None,
            },
            None => None,
        }
    }

    /// How long after `earlier` this instant is, or `None` when it is not after it.
    ///
    /// `None` also for a gap no [`Duration`] can hold. Every step is checked: a peer's
    /// timestamp is data, and two instants far enough apart to overflow an `i64` of
    /// seconds must not be an arithmetic panic in a driver that is answering one.
    #[must_use]
    #[allow(clippy::cast_sign_loss)] // `secs` is checked to be non-negative first
    pub const fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        let Some(secs) = self.secs.checked_sub(earlier.secs) else {
            return None;
        };
        let (secs, nanos) = if self.nanos >= earlier.nanos {
            (secs, self.nanos - earlier.nanos)
        } else {
            let Some(borrowed) = secs.checked_sub(1) else {
                return None;
            };
            (borrowed, self.nanos + 1_000_000_000 - earlier.nanos)
        };
        if secs < 0 {
            return None;
        }
        match (secs as u64).checked_mul(1_000) {
            Some(ms) => match ms.checked_add((nanos / 1_000_000) as u64) {
                Some(total) => Some(Duration::from_millis(total)),
                None => None,
            },
            None => None,
        }
    }

    /// How long after `earlier` this instant is, or zero when it is not after it.
    #[must_use]
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        match self.checked_duration_since(earlier) {
            Some(d) => d,
            None => Duration::ZERO,
        }
    }

    /// Parse an RFC 3339 date-time.
    pub fn parse(s: &str) -> Result<Self, InvalidTimestamp> {
        parse_rfc3339(s)
    }

    /// The civil date and time in UTC: `(year, month, day, hour, minute, second)`.
    #[must_use]
    pub const fn to_civil_utc(self) -> (i64, u32, u32, u32, u32, u32) {
        let days = self.secs.div_euclid(86_400);
        let sod = self.secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // sod is 0..86_399
        (
            y,
            m,
            d,
            (sod / 3_600) as u32,
            ((sod % 3_600) / 60) as u32,
            (sod % 60) as u32,
        )
    }
}

impl fmt::Display for Timestamp {
    #[allow(clippy::many_single_char_names)] // year, month, day, hour, minute, second
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, mo, d, h, mi, s) = self.to_civil_utc();
        if (0..=9999).contains(&y) {
            write!(f, "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")?;
        } else {
            // RFC 3339 has no spelling for these; emit an extended year rather than a
            // wrong one, so a round trip through our own parser still fails loudly.
            write!(f, "{y:+}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")?;
        }
        let n = self.nanos;
        if n != 0 {
            if n.is_multiple_of(1_000_000) {
                write!(f, ".{:03}", n / 1_000_000)?;
            } else if n.is_multiple_of(1_000) {
                write!(f, ".{:06}", n / 1_000)?;
            } else {
                write!(f, ".{n:09}")?;
            }
        }
        f.write_str("Z")
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({self})")
    }
}

impl FromStr for Timestamp {
    type Err = InvalidTimestamp;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_rfc3339(s)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use alloc::string::ToString;
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Timestamp;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an RFC 3339 date-time")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Timestamp, E> {
                parse_rfc3339(v).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Timestamp {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "DateTimeStamp".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "date-time" })
    }
}

// ---------------------------------------------------------------------------
// RFC 3339
// ---------------------------------------------------------------------------

fn digits(bytes: &[u8], from: usize, len: usize) -> Result<u64, InvalidTimestamp> {
    let slice = bytes
        .get(from..from + len)
        .ok_or(InvalidTimestamp::Malformed)?;
    let mut acc = 0u64;
    for &b in slice {
        if !b.is_ascii_digit() {
            return Err(InvalidTimestamp::Malformed);
        }
        acc = acc * 10 + u64::from(b - b'0');
    }
    Ok(acc)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]
fn parse_rfc3339(s: &str) -> Result<Timestamp, InvalidTimestamp> {
    let b = s.as_bytes();
    // The shortest legal form is `1970-01-01T00:00:00Z`: 20 bytes.
    if b.len() < 20 {
        return Err(InvalidTimestamp::Malformed);
    }
    let year = digits(b, 0, 4)? as i64;
    if b.get(4) != Some(&b'-') {
        return Err(InvalidTimestamp::Malformed);
    }
    let month = digits(b, 5, 2)? as u32;
    if b.get(7) != Some(&b'-') {
        return Err(InvalidTimestamp::Malformed);
    }
    let day = digits(b, 8, 2)? as u32;
    match b.get(10) {
        Some(&b'T' | &b't' | &b' ') => {}
        _ => return Err(InvalidTimestamp::Malformed),
    }
    let hour = digits(b, 11, 2)? as u32;
    if b.get(13) != Some(&b':') {
        return Err(InvalidTimestamp::Malformed);
    }
    let minute = digits(b, 14, 2)? as u32;
    if b.get(16) != Some(&b':') {
        return Err(InvalidTimestamp::Malformed);
    }
    let second = digits(b, 17, 2)? as u32;

    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') || b.get(i) == Some(&b',') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return Err(InvalidTimestamp::Malformed);
        }
        // Keep up to nine digits; anything finer than a nanosecond is truncated.
        let mut scale = 100_000_000u32;
        for &d in b.get(start..i).unwrap_or(&[]).iter().take(9) {
            nanos += u32::from(d - b'0') * scale;
            scale /= 10;
        }
    }

    let offset_secs: i64 = match b.get(i) {
        Some(&b'Z' | &b'z') => {
            i += 1;
            0
        }
        Some(&sign @ (b'+' | b'-')) => {
            let oh = digits(b, i + 1, 2)? as i64;
            if b.get(i + 3) != Some(&b':') {
                return Err(InvalidTimestamp::Malformed);
            }
            let om = digits(b, i + 4, 2)? as i64;
            if oh > 23 || om > 59 {
                return Err(InvalidTimestamp::FieldOutOfRange { field: "offset" });
            }
            i += 6;
            let magnitude = oh * 3_600 + om * 60;
            if sign == b'-' { -magnitude } else { magnitude }
        }
        _ => return Err(InvalidTimestamp::Malformed),
    };
    if i != b.len() {
        return Err(InvalidTimestamp::Malformed);
    }

    if !(1..=12).contains(&month) {
        return Err(InvalidTimestamp::FieldOutOfRange { field: "month" });
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(InvalidTimestamp::FieldOutOfRange { field: "day" });
    }
    if hour > 23 {
        return Err(InvalidTimestamp::FieldOutOfRange { field: "hour" });
    }
    if minute > 59 {
        return Err(InvalidTimestamp::FieldOutOfRange { field: "minute" });
    }
    // RFC 3339 permits `:60` for a leap second. Unix time has no room for one, so it
    // collapses onto :59 — the same thing every Unix-time library does.
    if second > 60 {
        return Err(InvalidTimestamp::FieldOutOfRange { field: "second" });
    }
    let second = second.min(59);

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)
        .and_then(|s| {
            s.checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
        })
        .and_then(|s| s.checked_sub(offset_secs))
        .ok_or(InvalidTimestamp::OutOfRange)?;
    Ok(Timestamp { secs, nanos })
}

const fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

const fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
///
/// The single-character names are the algorithm's own, and renaming them would make it
/// harder rather than easier to check against the published version.
#[allow(clippy::many_single_char_names)]
const fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
#[allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
const fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Conversions to the time crates people already use
// ---------------------------------------------------------------------------

#[cfg(feature = "jiff")]
#[cfg_attr(docsrs, doc(cfg(feature = "jiff")))]
mod jiff_conv {
    use super::{TimeOutOfRange, Timestamp};

    impl From<Timestamp> for jiff::Timestamp {
        fn from(t: Timestamp) -> Self {
            #[allow(clippy::cast_possible_wrap)]
            Self::new(t.unix_secs(), t.subsec_nanos() as i32).unwrap_or(Self::UNIX_EPOCH)
        }
    }

    impl TryFrom<jiff::Timestamp> for Timestamp {
        type Error = TimeOutOfRange;
        fn try_from(t: jiff::Timestamp) -> Result<Self, Self::Error> {
            let nanos = u32::try_from(t.subsec_nanosecond()).map_err(|_| TimeOutOfRange)?;
            Ok(Self::from_unix(t.as_second(), nanos))
        }
    }
}

#[cfg(feature = "time")]
#[cfg_attr(docsrs, doc(cfg(feature = "time")))]
mod time_conv {
    use super::{TimeOutOfRange, Timestamp};

    impl TryFrom<Timestamp> for time::OffsetDateTime {
        type Error = TimeOutOfRange;
        fn try_from(t: Timestamp) -> Result<Self, Self::Error> {
            let nanos = i128::from(t.unix_secs()) * 1_000_000_000 + i128::from(t.subsec_nanos());
            Self::from_unix_timestamp_nanos(nanos).map_err(|_| TimeOutOfRange)
        }
    }

    impl From<time::OffsetDateTime> for Timestamp {
        fn from(t: time::OffsetDateTime) -> Self {
            Self::from_unix(t.unix_timestamp(), t.nanosecond())
        }
    }
}

#[cfg(feature = "chrono")]
#[cfg_attr(docsrs, doc(cfg(feature = "chrono")))]
mod chrono_conv {
    use super::{TimeOutOfRange, Timestamp};

    impl TryFrom<Timestamp> for chrono::DateTime<chrono::Utc> {
        type Error = TimeOutOfRange;
        fn try_from(t: Timestamp) -> Result<Self, Self::Error> {
            Self::from_timestamp(t.unix_secs(), t.subsec_nanos()).ok_or(TimeOutOfRange)
        }
    }

    impl From<chrono::DateTime<chrono::Utc>> for Timestamp {
        fn from(t: chrono::DateTime<chrono::Utc>) -> Self {
            Self::from_unix(t.timestamp(), t.timestamp_subsec_nanos().min(999_999_999))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn every_arithmetic_operation_is_total_at_the_extremes() {
        // A peer's timestamp is data, and `Timestamp` is a public constructor, so the
        // extremes are reachable without anything being wrong. None of them may panic:
        // an overflow in a driver answering a peer is a crash, and in a debug build an
        // unchecked `i64` subtraction is one.
        let hi = Timestamp::from_unix(i64::MAX, 999_999_999);
        let lo = Timestamp::from_unix(i64::MIN, 0);

        assert_eq!(
            hi.checked_duration_since(lo),
            None,
            "gap overflows i64 secs"
        );
        assert_eq!(lo.checked_duration_since(hi), None, "not after");
        assert_eq!(hi.saturating_duration_since(lo), Duration::ZERO);
        assert_eq!(lo.saturating_duration_since(hi), Duration::ZERO);

        // The borrow path: `self.nanos < earlier.nanos` costs a second, and at the floor
        // there is no second to spend.
        let floor = Timestamp::from_unix(i64::MIN, 0);
        let just_above = Timestamp::from_unix(i64::MIN, 500);
        assert_eq!(floor.checked_duration_since(just_above), None);

        assert_eq!(hi.checked_add(Duration::from_millis(1)), None);
        assert_eq!(lo.checked_sub(Duration::from_millis(1_000)), None);

        // And the ordinary case still answers.
        let a = Timestamp::from_unix(1_000, 0);
        let b = Timestamp::from_unix(1_002, 500_000_000);
        assert_eq!(
            b.checked_duration_since(a),
            Some(Duration::from_millis(2_500))
        );
    }

    #[test]
    fn parses_every_timestamp_in_the_official_examples() {
        // Straight from docs `learn/examples/{ev,heat-pump,pv,nocontrol}`.
        for s in [
            "2019-08-24T14:15:22Z",
            "2024-08-24T14:15:22Z",
            "2024-08-24T15:00:00Z",
            "2024-12-25T14:15:22Z",
            "2024-08-24T14:00:00Z",
        ] {
            let t = Timestamp::parse(s).unwrap();
            assert_eq!(t.to_string(), s, "{s} should round-trip");
        }
    }

    #[test]
    fn accepts_the_rfc3339_spellings_a_peer_may_use() {
        let canonical = Timestamp::parse("2019-08-24T14:15:22Z").unwrap();
        for s in [
            "2019-08-24t14:15:22z",
            "2019-08-24 14:15:22Z",
            "2019-08-24T14:15:22+00:00",
            "2019-08-24T16:15:22+02:00",
            "2019-08-24T11:15:22-03:00",
            "2019-08-24T14:15:22.000Z",
            "2019-08-24T14:15:22.000000000Z",
        ] {
            assert_eq!(Timestamp::parse(s).unwrap(), canonical, "{s}");
        }
    }

    #[test]
    fn fractional_digits_are_preserved_and_minimised() {
        for (input, expected) in [
            ("2019-08-24T14:15:22.5Z", "2019-08-24T14:15:22.500Z"),
            ("2019-08-24T14:15:22.123Z", "2019-08-24T14:15:22.123Z"),
            ("2019-08-24T14:15:22.123456Z", "2019-08-24T14:15:22.123456Z"),
            (
                "2019-08-24T14:15:22.123456789Z",
                "2019-08-24T14:15:22.123456789Z",
            ),
            // Finer than a nanosecond is truncated, not rounded.
            (
                "2019-08-24T14:15:22.1234567891Z",
                "2019-08-24T14:15:22.123456789Z",
            ),
            ("2019-08-24T14:15:22.000Z", "2019-08-24T14:15:22Z"),
        ] {
            assert_eq!(Timestamp::parse(input).unwrap().to_string(), expected);
        }
    }

    #[test]
    fn rejects_what_is_not_a_date_time() {
        for s in [
            "",
            "2019-08-24",
            "2019-08-24T14:15:22",       // no offset
            "2019-13-24T14:15:22Z",      // month 13
            "2019-02-30T14:15:22Z",      // February 30
            "2019-08-24T25:15:22Z",      // hour 25
            "2019-08-24T14:60:22Z",      // minute 60
            "2019-08-24T14:15:61Z",      // second 61
            "2019-08-24T14:15:22Zjunk",  // trailing
            "2019-08-24T14:15:22.Z",     // empty fraction
            "2019-08-24T14:15:22+0200",  // offset without colon
            "2019-08-24T14:15:22+24:00", // offset out of range
        ] {
            assert!(Timestamp::parse(s).is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn leap_second_collapses_onto_fifty_nine() {
        // RFC 3339 allows it; Unix time has no room for it.
        let t = Timestamp::parse("2016-12-31T23:59:60Z").unwrap();
        assert_eq!(t.to_string(), "2016-12-31T23:59:59Z");
    }

    #[test]
    fn far_dates_survive_because_the_representation_is_not_nanoseconds() {
        // A peer may send a timer that finishes in the year 3000; a nanosecond count
        // would overflow in 2262 and this must be a warning rather than a parse failure.
        let t = Timestamp::parse("3000-01-01T00:00:00Z").unwrap();
        assert_eq!(t.to_string(), "3000-01-01T00:00:00Z");
        let t = Timestamp::parse("1900-01-01T00:00:00Z").unwrap();
        assert_eq!(t.to_string(), "1900-01-01T00:00:00Z");
        let t = Timestamp::parse("0001-01-01T00:00:00Z").unwrap();
        assert_eq!(t.to_string(), "0001-01-01T00:00:00Z");
        let t = Timestamp::parse("9999-12-31T23:59:59Z").unwrap();
        assert_eq!(t.to_string(), "9999-12-31T23:59:59Z");
    }

    #[test]
    fn civil_conversions_are_inverse_over_four_centuries() {
        // Every day from 1800 to 2200, there and back.
        let mut days = days_from_civil(1800, 1, 1);
        let end = days_from_civil(2200, 1, 1);
        while days < end {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
            days += 1;
        }
    }

    #[test]
    fn arithmetic_is_checked() {
        let t = Timestamp::parse("2019-08-24T14:15:22Z").unwrap();
        let hour = Duration::from_secs(3_600);
        assert_eq!(
            t.checked_add(hour).unwrap().to_string(),
            "2019-08-24T15:15:22Z"
        );
        assert_eq!(
            t.checked_sub(hour).unwrap().to_string(),
            "2019-08-24T13:15:22Z"
        );
        assert_eq!(
            t.checked_add(hour).unwrap().checked_duration_since(t),
            Some(hour)
        );
        assert_eq!(t.checked_duration_since(t.checked_add(hour).unwrap()), None);
        assert_eq!(
            t.saturating_duration_since(t.checked_add(hour).unwrap()),
            Duration::ZERO
        );
    }

    #[test]
    fn sub_second_arithmetic_carries() {
        let t = Timestamp::parse("2019-08-24T14:15:22.900Z").unwrap();
        assert_eq!(
            t.checked_add(Duration::from_millis(200))
                .unwrap()
                .to_string(),
            "2019-08-24T14:15:23.100Z"
        );
        let t = Timestamp::parse("2019-08-24T14:15:22.100Z").unwrap();
        assert_eq!(
            t.checked_sub(Duration::from_millis(200))
                .unwrap()
                .to_string(),
            "2019-08-24T14:15:21.900Z"
        );
    }

    #[test]
    fn ordering_is_chronological() {
        let a = Timestamp::parse("2019-08-24T14:15:22Z").unwrap();
        let b = Timestamp::parse("2019-08-24T14:15:22.001Z").unwrap();
        let c = Timestamp::parse("2019-08-24T14:15:23Z").unwrap();
        assert!(a < b && b < c);
    }

    #[test]
    fn json_is_a_bare_string() {
        let t = Timestamp::parse("2019-08-24T14:15:22Z").unwrap();
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            "\"2019-08-24T14:15:22Z\""
        );
        assert_eq!(
            serde_json::from_str::<Timestamp>("\"2019-08-24T14:15:22Z\"").unwrap(),
            t
        );
        assert!(serde_json::from_str::<Timestamp>("\"nope\"").is_err());
    }

    #[test]
    fn duration_is_milliseconds_on_the_wire() {
        let d = Duration::from_millis(3_600_000);
        assert_eq!(serde_json::to_string(&d).unwrap(), "3600000");
        assert_eq!(
            serde_json::from_str::<Duration>("0").unwrap(),
            Duration::ZERO
        );
        // The schema says minimum 0, and the type is unsigned, so a negative is refused.
        assert!(serde_json::from_str::<Duration>("-1").is_err());
    }

    #[test]
    fn size_is_what_we_promised() {
        assert_eq!(core::mem::size_of::<Timestamp>(), 16);
        assert_eq!(core::mem::size_of::<Duration>(), 8);
    }
}
