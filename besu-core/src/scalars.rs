//! SQL values that are not a Rust primitive: intervals, network addresses, bit strings,
//! ranges, spatial values and vectors.
//!
//! These live in `besu-core` because [`Value`] carries them. The
//! [`SqlType`](https://docs.rs/besu/latest/besu/trait.SqlType.html) implementations that name
//! them as column types live in `besu`, next to every other marker.
//!
//! Each type holds its value in the form the database spells it, so a decode and a re-encode
//! is lossless and comparison means what the database says it means:
//!
//! - [`SqlInterval`] keeps PostgreSQL's three fields — months, days and microseconds —
//!   because `'1 month'` and `'30 days'` are not the same interval.
//! - [`Inet`] and [`Cidr`] keep the canonical textual spelling.
//! - [`BitString`] keeps a string of `0` and `1`.
//! - [`Range`] keeps the two bounds, including whether each one is included, and whether the
//!   range is empty at all.
//! - [`Geometry`] keeps the EWKB bytes exactly as PostGIS sends them.
//! - [`Vector`] keeps the `f32` components a `pgvector` column holds.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::{fmt, str::FromStr};

/// A SQL `INTERVAL`, in PostgreSQL's three-field form.
///
/// An interval is not a duration: a month has no fixed number of days, and a day has no fixed
/// number of seconds in a time zone with daylight saving. PostgreSQL therefore stores three
/// independent fields, and this type keeps all three so a value written to the database comes
/// back with the same meaning.
///
/// The name avoids a clash with [`besu::Interval`](https://docs.rs/besu/latest/besu/expr/struct.Interval.html),
/// which builds an interval *literal* for date arithmetic (`date_add(now, Interval::days(1))`).
/// This type is the value a column holds.
///
/// ```ignore
/// let span = SqlInterval::new(0, 1, 90 * 60 * 1_000_000); // one day and ninety minutes
/// assert_eq!(span.to_string(), "1 days 01:30:00");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SqlInterval {
    /// Whole months. PostgreSQL's `year` is twelve months.
    months: i32,
    /// Whole days.
    days: i32,
    /// The time part, in microseconds.
    microseconds: i64,
}

impl SqlInterval {
    /// Build an interval from PostgreSQL's three fields.
    pub const fn new(months: i32, days: i32, microseconds: i64) -> Self {
        Self {
            months,
            days,
            microseconds,
        }
    }

    /// An interval that measures nothing.
    pub const fn zero() -> Self {
        Self::new(0, 0, 0)
    }

    /// An interval of whole months.
    pub const fn from_months(months: i32) -> Self {
        Self::new(months, 0, 0)
    }

    /// An interval of whole days.
    pub const fn from_days(days: i32) -> Self {
        Self::new(0, days, 0)
    }

    /// An interval of whole microseconds.
    pub const fn from_microseconds(microseconds: i64) -> Self {
        Self::new(0, 0, microseconds)
    }

    /// An interval of whole seconds.
    pub const fn from_seconds(seconds: i64) -> Self {
        Self::new(0, 0, seconds.saturating_mul(1_000_000))
    }

    /// Whole months.
    pub const fn months(&self) -> i32 {
        self.months
    }

    /// Whole days.
    pub const fn days(&self) -> i32 {
        self.days
    }

    /// The time part, in microseconds.
    pub const fn microseconds(&self) -> i64 {
        self.microseconds
    }

    /// Whether this interval measures nothing.
    pub const fn is_zero(&self) -> bool {
        self.months == 0 && self.days == 0 && self.microseconds == 0
    }
}

impl fmt::Display for SqlInterval {
    /// PostgreSQL's `postgres` output style: whole units first, then a time part.
    ///
    /// The spelling is canonical — `1 days 02:00:00`, never `1 day 2 hours` — so a value
    /// written to a text column and parsed back compares equal to the original.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote = false;
        if self.months != 0 {
            write!(f, "{} mons", self.months)?;
            wrote = true;
        }
        if self.days != 0 {
            if wrote {
                f.write_str(" ")?;
            }
            write!(f, "{} days", self.days)?;
            wrote = true;
        }
        if self.microseconds != 0 || !wrote {
            if wrote {
                f.write_str(" ")?;
            }
            let negative = self.microseconds < 0;
            // `unsigned_abs` is the one spelling that is correct at `i64::MIN`.
            let magnitude = self.microseconds.unsigned_abs();
            let hours = magnitude / 3_600_000_000;
            let minutes = (magnitude / 60_000_000) % 60;
            let seconds = (magnitude / 1_000_000) % 60;
            let micros = magnitude % 1_000_000;
            if negative {
                f.write_str("-")?;
            }
            write!(f, "{hours:02}:{minutes:02}:{seconds:02}")?;
            if micros != 0 {
                let mut fraction = alloc::format!(".{micros:06}");
                while fraction.ends_with('0') {
                    fraction.pop();
                }
                f.write_str(&fraction)?;
            }
        }
        Ok(())
    }
}

impl FromStr for SqlInterval {
    type Err = ParseValueError;

    /// Parse the canonical spelling, plus the unit names PostgreSQL accepts.
    ///
    /// Both `1 days 01:30:00` and `1 day 1 hour 30 minutes` are understood, and a bare
    /// `01:30:00` is the time part on its own. A unit may be abbreviated (`mon`, `min`,
    /// `sec`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ParseValueError::new(text, "an interval"));
        }

        let mut months: i64 = 0;
        let mut days: i64 = 0;
        let mut microseconds: i64 = 0;
        let mut parts = text.split_whitespace().peekable();

        while let Some(part) = parts.next() {
            if part.contains(':') {
                microseconds = add(microseconds, parse_time(part)?, text)?;
                continue;
            }

            let number: i64 = part
                .parse()
                .map_err(|_| ParseValueError::new(text, "an interval"))?;
            let unit = parts
                .next()
                .ok_or_else(|| ParseValueError::new(text, "an interval"))?
                .trim_end_matches(',')
                .to_ascii_lowercase();
            match unit.as_str() {
                "year" | "years" | "yr" | "yrs" | "y" => {
                    months = add(months, number.checked_mul(12).ok_or_else(overflow)?, text)?;
                }
                "month" | "months" | "mon" | "mons" => {
                    months = add(months, number, text)?;
                }
                "week" | "weeks" | "w" => {
                    days = add(days, number.checked_mul(7).ok_or_else(overflow)?, text)?;
                }
                "day" | "days" | "d" => {
                    days = add(days, number, text)?;
                }
                "hour" | "hours" | "hr" | "hrs" | "h" => {
                    microseconds = add(
                        microseconds,
                        number.checked_mul(3_600_000_000).ok_or_else(overflow)?,
                        text,
                    )?;
                }
                "minute" | "minutes" | "min" | "mins" | "m" => {
                    microseconds = add(
                        microseconds,
                        number.checked_mul(60_000_000).ok_or_else(overflow)?,
                        text,
                    )?;
                }
                "second" | "seconds" | "sec" | "secs" | "s" => {
                    microseconds = add(
                        microseconds,
                        number.checked_mul(1_000_000).ok_or_else(overflow)?,
                        text,
                    )?;
                }
                "millisecond" | "milliseconds" | "ms" => {
                    microseconds = add(
                        microseconds,
                        number.checked_mul(1_000).ok_or_else(overflow)?,
                        text,
                    )?;
                }
                "microsecond" | "microseconds" | "us" => {
                    microseconds = add(microseconds, number, text)?;
                }
                _ => return Err(ParseValueError::new(text, "an interval")),
            }
        }

        Ok(Self {
            months: i32::try_from(months).map_err(|_| ParseValueError::new(text, "an interval"))?,
            days: i32::try_from(days).map_err(|_| ParseValueError::new(text, "an interval"))?,
            microseconds,
        })
    }
}

/// The error for an `i64` arithmetic overflow while parsing an interval.
fn overflow() -> ParseValueError {
    ParseValueError::new("", "an interval")
}

/// Add two interval components, reporting overflow as a parse error.
fn add(left: i64, right: i64, text: &str) -> Result<i64, ParseValueError> {
    left.checked_add(right)
        .ok_or_else(|| ParseValueError::new(text, "an interval"))
}

/// Parse `[-]HH:MM[:SS[.ffffff]]` into microseconds.
fn parse_time(text: &str) -> Result<i64, ParseValueError> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let mut parts = rest.split(':');
    let hours: i64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| ParseValueError::new(text, "an interval"))?;
    let minutes: i64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| ParseValueError::new(text, "an interval"))?;
    let seconds = parts.next().unwrap_or("0");
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let whole: i64 = whole
        .parse()
        .map_err(|_| ParseValueError::new(text, "an interval"))?;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) || fraction.len() > 6 {
        return Err(ParseValueError::new(text, "an interval"));
    }
    let mut micros: i64 = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse()
            .map_err(|_| ParseValueError::new(text, "an interval"))?
    };
    for _ in fraction.len()..6 {
        micros *= 10;
    }

    let total = hours
        .checked_mul(3_600_000_000)
        .and_then(|value| value.checked_add(minutes.checked_mul(60_000_000)?))
        .and_then(|value| value.checked_add(whole.checked_mul(1_000_000)?))
        .and_then(|value| value.checked_add(micros))
        .ok_or_else(|| ParseValueError::new(text, "an interval"))?;
    Ok(if negative { -total } else { total })
}

impl From<chrono::Duration> for SqlInterval {
    /// Convert a fixed duration into days plus a time part.
    ///
    /// A duration has no months, so the month field stays zero: a duration of thirty days is
    /// not an interval of one month.
    fn from(value: chrono::Duration) -> Self {
        let days = value.num_days();
        let rest = value - chrono::Duration::days(days);
        Self {
            months: 0,
            days: i32::try_from(days).unwrap_or(if days < 0 { i32::MIN } else { i32::MAX }),
            microseconds: rest.num_microseconds().unwrap_or(0),
        }
    }
}

impl From<std::time::Duration> for SqlInterval {
    /// Convert a monotonic duration into whole days plus a time part.
    fn from(value: std::time::Duration) -> Self {
        let seconds = value.as_secs();
        let days = seconds / 86_400;
        let remainder = seconds % 86_400;
        Self {
            months: 0,
            days: i32::try_from(days).unwrap_or(i32::MAX),
            microseconds: i64::try_from(remainder)
                .unwrap_or(0)
                .saturating_mul(1_000_000)
                .saturating_add(i64::from(value.subsec_micros())),
        }
    }
}

/// A PostgreSQL `INET`: an address with an optional network prefix.
///
/// The value is kept in its canonical textual spelling, which is what PostgreSQL itself
/// reports. An address whose prefix covers the whole address is written without one —
/// `192.168.0.1` rather than `192.168.0.1/32` — because that is how PostgreSQL prints an
/// `INET`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Inet(String);

impl Inet {
    /// Parse an address, canonicalising it the way PostgreSQL does.
    ///
    /// The prefix is optional and defaults to the whole address.
    pub fn new(text: &str) -> Result<Self, ParseValueError> {
        let (address, prefix) = parse_address(text, "INET")?;
        Ok(Self(format_address(address, prefix, true)))
    }

    /// The canonical spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The address, without the prefix.
    pub fn address(&self) -> &str {
        self.0.split_once('/').map_or(self.0.as_str(), |(a, _)| a)
    }

    /// The prefix length.
    pub fn prefix(&self) -> u8 {
        prefix_of(&self.0)
    }
}

impl fmt::Display for Inet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Inet {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for Inet {
    type Err = ParseValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl TryFrom<String> for Inet {
    type Error = ParseValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

/// A PostgreSQL `CIDR`: a network, always written with its prefix.
///
/// Unlike [`Inet`], the host bits are cleared on construction, because a network address
/// does not have any: `192.168.1.7/24` becomes `192.168.1.0/24`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cidr(String);

impl Cidr {
    /// Parse a network, clearing any host bits below the prefix.
    pub fn new(text: &str) -> Result<Self, ParseValueError> {
        let (address, prefix) = parse_address(text, "CIDR")?;
        Ok(Self(format_address(
            mask_address(address, prefix),
            prefix,
            false,
        )))
    }

    /// The canonical spelling, which always includes the prefix.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The network address, without the prefix.
    pub fn address(&self) -> &str {
        self.0.split_once('/').map_or(self.0.as_str(), |(a, _)| a)
    }

    /// The prefix length.
    pub fn prefix(&self) -> u8 {
        prefix_of(&self.0)
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Cidr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for Cidr {
    type Err = ParseValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl TryFrom<String> for Cidr {
    type Error = ParseValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

/// Split `address[/prefix]` and validate the prefix against the address family.
fn parse_address(
    text: &str,
    expected: &'static str,
) -> Result<(core::net::IpAddr, u8), ParseValueError> {
    let text = text.trim();
    let (address, prefix) = match text.split_once('/') {
        Some((address, prefix)) => (
            address,
            Some(
                prefix
                    .parse::<u8>()
                    .map_err(|_| ParseValueError::new(text, expected))?,
            ),
        ),
        None => (text, None),
    };
    let address: core::net::IpAddr = address
        .parse()
        .map_err(|_| ParseValueError::new(text, expected))?;
    let max = match address {
        core::net::IpAddr::V4(_) => 32,
        core::net::IpAddr::V6(_) => 128,
    };
    let prefix = prefix.unwrap_or(max);
    if prefix > max {
        return Err(ParseValueError::new(text, expected));
    }
    Ok((address, prefix))
}

/// The prefix length of an already-canonical spelling.
fn prefix_of(text: &str) -> u8 {
    text.rsplit_once('/')
        .and_then(|(_, prefix)| prefix.parse().ok())
        .unwrap_or_else(|| if text.contains(':') { 128 } else { 32 })
}

/// Format an address, omitting a full prefix when `omit_full` is set.
fn format_address(address: core::net::IpAddr, prefix: u8, omit_full: bool) -> String {
    let full = match address {
        core::net::IpAddr::V4(_) => 32,
        core::net::IpAddr::V6(_) => 128,
    };
    if omit_full && prefix == full {
        address.to_string()
    } else {
        alloc::format!("{address}/{prefix}")
    }
}

/// Clear the bits below `prefix`, so the address is a network.
fn mask_address(address: core::net::IpAddr, prefix: u8) -> core::net::IpAddr {
    match address {
        core::net::IpAddr::V4(address) => {
            let bits = u32::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            core::net::IpAddr::V4((bits & mask).into())
        }
        core::net::IpAddr::V6(address) => {
            let bits = u128::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            core::net::IpAddr::V6((bits & mask).into())
        }
    }
}

/// A SQL bit string: a sequence of `0` and `1`.
///
/// PostgreSQL's `BIT(n)` and `BIT VARYING(n)` are the types with a wire format of their own;
/// MySQL's `BIT(n)` is sent as bytes and SQLite has no bit type at all, so it stores this
/// string as text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BitString(String);

impl BitString {
    /// A bit string with no bits.
    pub const fn empty() -> Self {
        Self(String::new())
    }

    /// Build a bit string from a string of `0` and `1`.
    pub fn new(text: impl Into<String>) -> Result<Self, ParseValueError> {
        let text = text.into();
        if text.bytes().all(|byte| matches!(byte, b'0' | b'1')) {
            Ok(Self(text))
        } else {
            Err(ParseValueError::new(text, "a bit string"))
        }
    }

    /// Build a bit string from one flag per bit, most significant first.
    pub fn from_bits(bits: impl IntoIterator<Item = bool>) -> Self {
        Self(
            bits.into_iter()
                .map(|bit| if bit { '1' } else { '0' })
                .collect(),
        )
    }

    /// Build a bit string from bytes, most significant bit first.
    ///
    /// `length` is the number of bits to keep, so a value shorter than eight bits drops the
    /// padding at the end of the last byte.
    pub fn from_bytes(bytes: &[u8], length: usize) -> Result<Self, ParseValueError> {
        if length > bytes.len() * 8 {
            return Err(ParseValueError::new("", "a bit string"));
        }
        let mut text = String::with_capacity(length);
        for index in 0..length {
            let byte = bytes[index / 8];
            let bit = (byte >> (7 - index % 8)) & 1;
            text.push(if bit == 1 { '1' } else { '0' });
        }
        Ok(Self(text))
    }

    /// The bits as a string of `0` and `1`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// How many bits there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no bits.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The bit at `index`, if there is one.
    pub fn bit(&self, index: usize) -> Option<bool> {
        self.0.as_bytes().get(index).map(|bit| *bit == b'1')
    }

    /// The bits, in order.
    pub fn bits(&self) -> impl Iterator<Item = bool> + '_ {
        self.0.bytes().map(|bit| bit == b'1')
    }

    /// The bits packed into bytes, most significant bit first, zero-padded at the end.
    ///
    /// This is the payload PostgreSQL's `bit` and `varbit` receive functions expect after
    /// their length header, and the payload MySQL's `BIT(n)` expects.
    pub fn to_bytes(&self) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec![0u8; self.0.len().div_ceil(8)];
        for (index, bit) in self.0.bytes().enumerate() {
            if bit == b'1' {
                bytes[index / 8] |= 1 << (7 - index % 8);
            }
        }
        bytes
    }
}

impl fmt::Display for BitString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for BitString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for BitString {
    type Err = ParseValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl TryFrom<String> for BitString {
    type Error = ParseValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for BitString {
    type Error = ParseValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// One end of a [`Range`].
///
/// PostgreSQL spells the two finite forms with a bracket and an infinite one with a
/// parenthesis, which is also how [`Display`](fmt::Display) writes a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bound<T> {
    /// The bound is part of the range: `[a` or `b]`.
    Included(T),
    /// The bound is not part of the range: `(a` or `b)`.
    Excluded(T),
    /// There is no bound: `(,` or `,)`.
    Unbounded,
}

impl<T> Bound<T> {
    /// The bound's value, or `None` when it is unbounded.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Included(value) | Self::Excluded(value) => Some(value),
            Self::Unbounded => None,
        }
    }

    /// Convert the bound's value, keeping whether it is included.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Bound<U> {
        match self {
            Self::Included(value) => Bound::Included(f(value)),
            Self::Excluded(value) => Bound::Excluded(f(value)),
            Self::Unbounded => Bound::Unbounded,
        }
    }
}

/// A SQL range: PostgreSQL's `int4range`, `tsrange` and friends.
///
/// The two bounds are kept as they were declared, because inclusive and exclusive bounds are
/// different values: `[1,5)` contains `1` and `(1,5)` does not. A range can also be
/// [`empty`](Range::empty), which is a value of its own rather than a range with no bounds.
///
/// The type deliberately does not reuse [`core::ops::Range`]: that is a half-open range with
/// no notion of inclusion, no unbounded end and no empty value, and it cannot describe the
/// SQL type.
///
/// ```ignore
/// let ids = Range::closed(1, 5);            // [1,5]
/// let window = Range::new(Bound::Included(1), Bound::Excluded(5)); // [1,5)
/// assert!(window.contains(&2));
/// assert!(!window.contains(&5));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Range<T> {
    /// `None` is the empty range; `Some` holds the lower and upper bound.
    bounds: Option<(Bound<T>, Bound<T>)>,
}

impl<T> Default for Range<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> Range<T> {
    /// A range with the given bounds.
    pub const fn new(start: Bound<T>, end: Bound<T>) -> Self {
        Self {
            bounds: Some((start, end)),
        }
    }

    /// A range with no values at all, which PostgreSQL writes as `empty`.
    pub const fn empty() -> Self {
        Self { bounds: None }
    }

    /// A range that includes both endpoints: `[start,end]`.
    pub fn closed(start: T, end: T) -> Self {
        Self::new(Bound::Included(start), Bound::Included(end))
    }

    /// A range that excludes both endpoints: `(start,end)`.
    pub fn open(start: T, end: T) -> Self {
        Self::new(Bound::Excluded(start), Bound::Excluded(end))
    }

    /// A range that includes its start and excludes its end: `[start,end)`.
    pub fn closed_open(start: T, end: T) -> Self {
        Self::new(Bound::Included(start), Bound::Excluded(end))
    }

    /// A range that excludes its start and includes its end: `(start,end]`.
    pub fn open_closed(start: T, end: T) -> Self {
        Self::new(Bound::Excluded(start), Bound::Included(end))
    }

    /// Whether this is the empty range.
    pub const fn is_empty(&self) -> bool {
        self.bounds.is_none()
    }

    /// The lower and upper bound, or `None` when the range is empty.
    pub fn bounds(&self) -> Option<(&Bound<T>, &Bound<T>)> {
        self.bounds.as_ref().map(|(start, end)| (start, end))
    }

    /// The lower bound, or `None` when the range is empty.
    pub fn start(&self) -> Option<&Bound<T>> {
        self.bounds.as_ref().map(|(start, _)| start)
    }

    /// The upper bound, or `None` when the range is empty.
    pub fn end(&self) -> Option<&Bound<T>> {
        self.bounds.as_ref().map(|(_, end)| end)
    }

    /// Convert both bounds' values.
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Range<U> {
        match self.bounds {
            None => Range::empty(),
            Some((start, end)) => Range::new(start.map(&mut f), end.map(&mut f)),
        }
    }

    /// Whether `value` lies inside the range.
    ///
    /// The comparison is the element's own ordering, exactly as PostgreSQL's `range @>`
    /// operator uses it.
    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialOrd,
    {
        let Some((start, end)) = &self.bounds else {
            return false;
        };
        let after_start = match start {
            Bound::Included(start) => value >= start,
            Bound::Excluded(start) => value > start,
            Bound::Unbounded => true,
        };
        let before_end = match end {
            Bound::Included(end) => value <= end,
            Bound::Excluded(end) => value < end,
            Bound::Unbounded => true,
        };
        after_start && before_end
    }

    /// Build a range from PostgreSQL's `[a,b)` spelling, decoding each finite bound with
    /// `parse`.
    ///
    /// A bound wrapped in double quotes is unquoted, with `""` becoming a literal quote, so
    /// the spelling PostgreSQL writes for a text range is understood as well as the one for a
    /// numeric range.
    pub fn parse_with<E>(
        text: &str,
        mut parse: impl FnMut(&str) -> Result<T, E>,
    ) -> Result<Self, ParseValueError> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("empty") {
            return Ok(Self::empty());
        }

        let (start_bracket, rest) = match text.chars().next() {
            Some(bracket @ ('[' | '(')) => (bracket, &text[bracket.len_utf8()..]),
            _ => return Err(ParseValueError::new(text, "a range")),
        };
        let (rest, end_bracket) = match rest.chars().last() {
            Some(bracket @ (']' | ')')) => (&rest[..rest.len() - bracket.len_utf8()], bracket),
            _ => return Err(ParseValueError::new(text, "a range")),
        };

        let (start, end) =
            split_bounds(rest).ok_or_else(|| ParseValueError::new(text, "a range"))?;
        let start = if start.is_empty() {
            Bound::Unbounded
        } else {
            let value =
                parse(&unquote(start)).map_err(|_| ParseValueError::new(text, "a range"))?;
            if start_bracket == '[' {
                Bound::Included(value)
            } else {
                Bound::Excluded(value)
            }
        };
        let end = if end.is_empty() {
            Bound::Unbounded
        } else {
            let value = parse(&unquote(end)).map_err(|_| ParseValueError::new(text, "a range"))?;
            if end_bracket == ']' {
                Bound::Included(value)
            } else {
                Bound::Excluded(value)
            }
        };
        Ok(Self::new(start, end))
    }
}

impl<T: fmt::Display> fmt::Display for Range<T> {
    /// The canonical spelling: `empty`, `[1,5)`, `(,5]`, `[1,)`.
    ///
    /// Numeric, date and timestamp bounds are written exactly as PostgreSQL writes them; a
    /// text bound is written bare, because the ranges Besu models have no text subtype.
    /// [`parse_with`](Range::parse_with) accepts a quoted bound as well.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((start, end)) = &self.bounds else {
            return f.write_str("empty");
        };
        f.write_str(match start {
            Bound::Included(_) => "[",
            Bound::Excluded(_) | Bound::Unbounded => "(",
        })?;
        if let Some(value) = start.value() {
            write!(f, "{value}")?;
        }
        f.write_str(",")?;
        if let Some(value) = end.value() {
            write!(f, "{value}")?;
        }
        f.write_str(match end {
            Bound::Included(_) => "]",
            Bound::Excluded(_) | Bound::Unbounded => ")",
        })
    }
}

impl<T: FromStr> FromStr for Range<T> {
    type Err = ParseValueError;

    /// Parse PostgreSQL's `[a,b)` spelling, or `empty`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse_with(text, str::parse)
    }
}

/// Split `a,b` at the comma that is not inside a quoted bound.
fn split_bounds(text: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => return Some((&text[..index], &text[index + 1..])),
            _ => {}
        }
    }
    None
}

/// Strip the double quotes PostgreSQL wraps around a bound that needs them.
fn unquote(text: &str) -> String {
    let Some(inner) = text
        .trim()
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    else {
        return text.trim().to_owned();
    };
    inner.replace("\"\"", "\"").replace("\\\\", "\\")
}

/// The shape of a spatial column, as PostgreSQL's `geometry(Point,4326)` names it.
///
/// PostGIS reports the shape as part of the column's type, so it is part of the IR: a
/// `GEOMETRY(POINT)` column and a plain `GEOMETRY` column accept different values. A shape
/// Besu does not know — `PointZ` or `PolyhedralSurface`, for instance — is kept verbatim, so
/// a pulled schema renders the type it found.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum GeometryKind {
    /// Any geometry: the bare `GEOMETRY`/`GEOGRAPHY` type.
    Geometry,
    /// A point.
    Point,
    /// A line string.
    LineString,
    /// A polygon.
    Polygon,
    /// A collection of points.
    MultiPoint,
    /// A collection of line strings.
    MultiLineString,
    /// A collection of polygons.
    MultiPolygon,
    /// A collection of mixed geometries.
    GeometryCollection,
    /// A shape Besu does not name, kept exactly as the database spelled it.
    Other(String),
}

impl GeometryKind {
    /// The shape as PostGIS spells it: `Point`, `LineString`, and so on.
    pub fn name(&self) -> &str {
        match self {
            Self::Geometry => "Geometry",
            Self::Point => "Point",
            Self::LineString => "LineString",
            Self::Polygon => "Polygon",
            Self::MultiPoint => "MultiPoint",
            Self::MultiLineString => "MultiLineString",
            Self::MultiPolygon => "MultiPolygon",
            Self::GeometryCollection => "GeometryCollection",
            Self::Other(name) => name,
        }
    }

    /// Parse a shape name, matching the seven PostGIS names case-insensitively.
    ///
    /// Anything else is kept in [`GeometryKind::Other`], so a shape from a newer PostGIS is
    /// described rather than dropped.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "geometry" => Self::Geometry,
            "point" => Self::Point,
            "linestring" => Self::LineString,
            "polygon" => Self::Polygon,
            "multipoint" => Self::MultiPoint,
            "multilinestring" => Self::MultiLineString,
            "multipolygon" => Self::MultiPolygon,
            "geometrycollection" => Self::GeometryCollection,
            _ => Self::Other(name.trim().to_owned()),
        }
    }

    /// The shape named by one of the fixed codes a `SqlType` marker carries.
    ///
    /// This exists so a marker's type definition can stay a constant: a shape built from an
    /// enum discriminant is a constant, while one built by matching a string is not.
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            4 => Self::MultiPoint,
            5 => Self::MultiLineString,
            6 => Self::MultiPolygon,
            7 => Self::GeometryCollection,
            _ => Self::Geometry,
        }
    }
}

impl fmt::Display for GeometryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A spatial value, kept as PostGIS's extended well-known binary (EWKB).
///
/// EWKB is what PostGIS sends and accepts, and it is the only form that carries every shape:
/// a byte-order marker, a four-byte type word with an optional SRID flag, an optional
/// four-byte SRID and the shape's payload. Keeping the bytes rather than parsing them means a
/// value round-trips exactly, including shapes Besu does not name.
///
/// MySQL stores the shape as a four-byte SRID followed by plain WKB; the driver converts
/// between the two forms, so a `Geometry` written on one server describes the same shape on
/// the other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Geometry(Vec<u8>);

impl Geometry {
    /// The flag EWKB sets to say an SRID follows the type word.
    const SRID_FLAG: u32 = 0x2000_0000;

    /// Wrap EWKB bytes, checking that they start with a header Besu can describe.
    ///
    /// The payload itself is not validated: a shape Besu does not know is still a value it
    /// can store and return, and rejecting it here would make a newer PostGIS unusable.
    pub fn from_ewkb(bytes: impl Into<Vec<u8>>) -> Result<Self, ParseValueError> {
        let bytes = bytes.into();
        if ewkb_header(&bytes).is_none() {
            return Err(ParseValueError::new("", "extended well-known binary"));
        }
        Ok(Self(bytes))
    }

    /// Wrap EWKB bytes without validating the header.
    ///
    /// The server produced these bytes and will receive them unchanged, so a shape Besu
    /// cannot parse is not a reason to fail a decode.
    pub fn from_ewkb_unchecked(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// Wrap the hex spelling of EWKB, which is what PostGIS's output function writes.
    pub fn from_hex(text: &str) -> Result<Self, ParseValueError> {
        let text = text.trim();
        if text.len() % 2 != 0 {
            return Err(ParseValueError::new(text, "hexadecimal EWKB"));
        }
        let mut bytes = Vec::with_capacity(text.len() / 2);
        let digits = text.as_bytes();
        let nibble = |byte: u8| -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            }
        };
        for pair in digits.chunks_exact(2) {
            let high =
                nibble(pair[0]).ok_or_else(|| ParseValueError::new(text, "hexadecimal EWKB"))?;
            let low =
                nibble(pair[1]).ok_or_else(|| ParseValueError::new(text, "hexadecimal EWKB"))?;
            bytes.push((high << 4) | low);
        }
        Self::from_ewkb(bytes)
    }

    /// The EWKB bytes, exactly as they were received.
    pub fn ewkb(&self) -> &[u8] {
        &self.0
    }

    /// Take the EWKB bytes.
    pub fn into_ewkb(self) -> Vec<u8> {
        self.0
    }

    /// The hex spelling of the EWKB bytes, matching PostGIS's text output.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let mut out = String::with_capacity(self.0.len() * 2);
        for byte in &self.0 {
            out.push(char::from(DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        out
    }

    /// The SRID the shape carries, when the encoding names one.
    pub fn srid(&self) -> Option<u32> {
        let header = ewkb_header(&self.0)?;
        if header.srid {
            // The SRID is written in the geometry's own byte order, like every other number
            // in the value.
            let bytes = self.0.get(5..9)?;
            Some(match self.0[0] {
                0 => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                _ => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            })
        } else {
            None
        }
    }

    /// The shape the header names, or `None` when the type word is not one Besu knows.
    pub fn kind(&self) -> Option<GeometryKind> {
        let header = ewkb_header(&self.0)?;
        Some(match header.kind {
            1 => GeometryKind::Point,
            2 => GeometryKind::LineString,
            3 => GeometryKind::Polygon,
            4 => GeometryKind::MultiPoint,
            5 => GeometryKind::MultiLineString,
            6 => GeometryKind::MultiPolygon,
            7 => GeometryKind::GeometryCollection,
            _ => return None,
        })
    }

    /// Convert to MySQL's internal geometry format: a four-byte little-endian SRID followed
    /// by plain WKB.
    ///
    /// MySQL keeps the SRID beside the WKB rather than inside it; PostGIS keeps it inside the
    /// EWKB type word. Both describe the same shape, so a `Geometry` is converted rather than
    /// re-encoded when it crosses between them.
    pub fn to_mysql_bytes(&self) -> Vec<u8> {
        let srid = self.srid().unwrap_or(0);
        let mut out = Vec::with_capacity(self.0.len() + 4);
        out.extend_from_slice(&srid.to_le_bytes());
        match ewkb_header(&self.0) {
            Some(header) if header.srid => {
                let order = self.0[0];
                let word = &self.0[1..5];
                let type_word = match order {
                    0 => u32::from_be_bytes([word[0], word[1], word[2], word[3]]),
                    _ => u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
                } & !Self::SRID_FLAG;
                out.push(order);
                match order {
                    0 => out.extend_from_slice(&type_word.to_be_bytes()),
                    _ => out.extend_from_slice(&type_word.to_le_bytes()),
                }
                // Skip the four SRID bytes that followed the type word.
                out.extend_from_slice(self.0.get(9..).unwrap_or_default());
            }
            _ => out.extend_from_slice(&self.0),
        }
        out
    }

    /// Read MySQL's internal geometry format: a four-byte little-endian SRID followed by
    /// plain WKB, converted to EWKB.
    pub fn from_mysql_bytes(bytes: &[u8]) -> Result<Self, ParseValueError> {
        let srid = bytes
            .get(..4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .ok_or_else(|| ParseValueError::new("", "a MySQL geometry"))?;
        let wkb = &bytes[4..];
        let header =
            ewkb_header(wkb).ok_or_else(|| ParseValueError::new("", "a MySQL geometry"))?;
        if header.srid || srid == 0 {
            // MySQL can return the SRID inside the WKB as well, and an SRID of zero means the
            // geometry does not carry one.
            return Self::from_ewkb(wkb.to_vec());
        }

        let order = wkb[0];
        let word = &wkb[1..5];
        let type_word = match order {
            0 => u32::from_be_bytes([word[0], word[1], word[2], word[3]]),
            _ => u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
        } | Self::SRID_FLAG;
        let mut out = Vec::with_capacity(wkb.len() + 4);
        out.push(order);
        match order {
            0 => out.extend_from_slice(&type_word.to_be_bytes()),
            _ => out.extend_from_slice(&type_word.to_le_bytes()),
        }
        // The SRID follows in the same byte order as the rest of the value.
        match order {
            0 => out.extend_from_slice(&srid.to_be_bytes()),
            _ => out.extend_from_slice(&srid.to_le_bytes()),
        }
        out.extend_from_slice(&wkb[5..]);
        Self::from_ewkb(out)
    }
}

impl AsRef<[u8]> for Geometry {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Geometry> for Vec<u8> {
    fn from(value: Geometry) -> Self {
        value.0
    }
}

impl fmt::Display for Geometry {
    /// The hex EWKB spelling, which is what PostGIS's own output function writes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for Geometry {
    type Err = ParseValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_hex(text)
    }
}

impl TryFrom<Vec<u8>> for Geometry {
    type Error = ParseValueError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::from_ewkb(value)
    }
}

/// The parts of an EWKB header, or `None` when the bytes do not start with one.
struct EwkbHeader {
    /// Whether a four-byte SRID follows the type word.
    srid: bool,
    /// The shape's type number.
    kind: u32,
}

/// Read the byte order, type word and SRID flag of an EWKB value.
fn ewkb_header(bytes: &[u8]) -> Option<EwkbHeader> {
    let order = *bytes.first()?;
    let word = bytes.get(1..5)?;
    let type_word = match order {
        // XDR: big-endian.
        0 => u32::from_be_bytes([word[0], word[1], word[2], word[3]]),
        // NDR: little-endian.
        1 => u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
        _ => return None,
    };
    // The type word carries the SRID flag plus the Z and M flags PostGIS uses; the shape is
    // what remains after masking them off.
    let srid = type_word & Geometry::SRID_FLAG != 0;
    let kind = type_word & !(Geometry::SRID_FLAG | 0x8000_0000 | 0x4000_0000);
    // A shape Besu does not name — a circular string, say — is still a value it can store and
    // return, so only the impossible type zero is rejected.
    if kind == 0 {
        return None;
    }
    if srid && bytes.len() < 9 {
        return None;
    }
    Some(EwkbHeader { srid, kind })
}

/// A `pgvector` vector: a list of `f32` components.
///
/// On PostgreSQL the value is sent untyped in `pgvector`'s binary form — a two-byte dimension,
/// two unused bytes and the components as big-endian `f32`s — so the server infers the column's
/// exact `vector(n)` type and checks the dimension itself. A result is read from that same form,
/// with the `[1,2,3]` text accepted as a fallback. MySQL and SQLite store the text, which is what
/// `Display` writes.
///
/// A `Vec<f32>` field stays a PostgreSQL array; use this type (or
/// [`VectorOf`](https://docs.rs/besu/latest/besu/type.VectorOf.html)) to mean a vector
/// column.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vector(Vec<f32>);

impl Vector {
    /// Build a vector from its components.
    pub fn new(values: impl Into<Vec<f32>>) -> Self {
        Self(values.into())
    }

    /// The components.
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// How many components there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no components.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Take the components.
    pub fn into_inner(self) -> Vec<f32> {
        self.0
    }

    /// Read `pgvector`'s binary form: a two-byte dimension, two unused bytes and the
    /// components as big-endian `f32`s.
    pub fn from_binary(bytes: &[u8]) -> Result<Self, ParseValueError> {
        fn malformed() -> ParseValueError {
            ParseValueError::new("", "a binary vector")
        }

        let header = bytes.get(..4).ok_or_else(malformed)?;
        let dimension = i16::from_be_bytes([header[0], header[1]]);
        let dimension = usize::try_from(dimension).map_err(|_| malformed())?;
        let payload = bytes.get(4..).ok_or_else(malformed)?;
        if payload.len() != dimension * 4 {
            return Err(malformed());
        }
        let values = payload
            .chunks_exact(4)
            .map(|value| f32::from_be_bytes([value[0], value[1], value[2], value[3]]))
            .collect();
        Ok(Self(values))
    }

    /// Write `pgvector`'s binary form, which is what the extension sends inside a query
    /// result.
    pub fn to_binary(&self) -> Vec<u8> {
        let dimension = i16::try_from(self.0.len()).unwrap_or(i16::MAX);
        let components = usize::try_from(dimension).unwrap_or(0);
        let mut out = Vec::with_capacity(4 + components * 4);
        out.extend_from_slice(&dimension.to_be_bytes());
        // The second field is unused by the extension and is always zero.
        out.extend_from_slice(&0_i16.to_be_bytes());
        for value in self.0.iter().take(components) {
            out.extend_from_slice(&value.to_be_bytes());
        }
        out
    }
}

impl From<Vec<f32>> for Vector {
    fn from(value: Vec<f32>) -> Self {
        Self(value)
    }
}

impl From<Vector> for Vec<f32> {
    fn from(value: Vector) -> Self {
        value.0
    }
}

impl AsRef<[f32]> for Vector {
    fn as_ref(&self) -> &[f32] {
        &self.0
    }
}

impl fmt::Display for Vector {
    /// `pgvector`'s text spelling: `[1,2,3]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (index, value) in self.0.iter().enumerate() {
            if index != 0 {
                f.write_str(",")?;
            }
            write!(f, "{value}")?;
        }
        f.write_str("]")
    }
}

impl FromStr for Vector {
    type Err = ParseValueError;

    /// Parse `[1,2,3]`, tolerating spaces after the commas.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let inner = text
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
            .ok_or_else(|| ParseValueError::new(text, "a vector"))?;
        if inner.trim().is_empty() {
            return Ok(Self::new(Vec::new()));
        }
        let values = inner
            .split(',')
            .map(|value| {
                value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| ParseValueError::new(text, "a vector"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(values))
    }
}

/// Why a textual value could not be parsed into one of the types in this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseValueError {
    value: String,
    expected: &'static str,
}

impl ParseValueError {
    /// Record the text that failed and what it was supposed to be.
    pub fn new(value: impl Into<String>, expected: &'static str) -> Self {
        Self {
            value: value.into(),
            expected,
        }
    }

    /// The text that failed.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// What the text was supposed to be, as in "an interval".
    pub fn expected(&self) -> &'static str {
        self.expected
    }
}

impl fmt::Display for ParseValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.value.is_empty() {
            write!(f, "the value is not {}", self.expected)
        } else {
            write!(f, "`{}` is not {}", self.value, self.expected)
        }
    }
}

impl core::error::Error for ParseValueError {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn an_interval_prints_and_parses_its_canonical_form() {
        let cases = [
            (SqlInterval::zero(), "00:00:00"),
            (SqlInterval::new(0, 1, 0), "1 days"),
            (
                SqlInterval::new(26, 3, 5_400_000_000),
                "26 mons 3 days 01:30:00",
            ),
            (SqlInterval::new(0, 0, -90_500_000), "-00:01:30.5"),
            (SqlInterval::new(0, -2, 0), "-2 days"),
        ];
        for (value, text) in cases {
            assert_eq!(value.to_string(), text);
            assert_eq!(text.parse::<SqlInterval>(), Ok(value), "{text}");
        }
    }

    #[test]
    fn an_interval_parses_units_and_a_time_part() {
        assert_eq!(
            "1 year 2 mons 3 days 04:05:06.789".parse::<SqlInterval>(),
            Ok(SqlInterval::new(
                14,
                3,
                4 * 3_600_000_000 + 5 * 60_000_000 + 6_789_000
            ))
        );
        assert_eq!(
            "1 week 2 hours".parse::<SqlInterval>(),
            Ok(SqlInterval::new(0, 7, 7_200_000_000))
        );
        assert!("one day".parse::<SqlInterval>().is_err());
        assert!("1 fortnight".parse::<SqlInterval>().is_err());
    }

    #[test]
    fn durations_become_intervals_without_months() {
        let interval = SqlInterval::from(chrono::Duration::seconds(90_000));
        assert_eq!(interval, SqlInterval::new(0, 1, 3_600_000_000));
        assert_eq!(interval.months(), 0);

        let interval = SqlInterval::from(std::time::Duration::from_secs(90_000));
        assert_eq!(interval, SqlInterval::new(0, 1, 3_600_000_000));
    }

    #[test]
    fn an_inet_keeps_a_host_and_omits_a_full_prefix() {
        assert_eq!(Inet::new("192.168.0.1").unwrap().to_string(), "192.168.0.1");
        assert_eq!(
            Inet::new("192.168.0.1/32").unwrap().to_string(),
            "192.168.0.1"
        );
        assert_eq!(Inet::new("10.0.0.1/8").unwrap().to_string(), "10.0.0.1/8");
        assert_eq!(
            Inet::new("2001:db8::1/64").unwrap().to_string(),
            "2001:db8::1/64"
        );
        assert!(Inet::new("10.0.0.1/33").is_err());
        assert!(Inet::new("not an address").is_err());
    }

    #[test]
    fn a_cidr_clears_host_bits_and_always_names_its_prefix() {
        assert_eq!(
            Cidr::new("192.168.1.7/24").unwrap().to_string(),
            "192.168.1.0/24"
        );
        assert_eq!(
            Cidr::new("192.168.1.7").unwrap().to_string(),
            "192.168.1.7/32"
        );
        assert_eq!(Cidr::new("10.0.0.0/8").unwrap().prefix(), 8);
        assert_eq!(
            Cidr::new("2001:db8::1/64").unwrap().to_string(),
            "2001:db8::/64"
        );
    }

    #[test]
    fn a_bit_string_packs_bits_most_significant_first() {
        let bits = BitString::new("10110011").unwrap();
        assert_eq!(bits.to_bytes(), vec![0b1011_0011]);
        assert_eq!(BitString::from_bytes(&[0b1011_0011], 8).unwrap(), bits);
        assert_eq!(
            BitString::from_bytes(&[0b1011_0011], 5).unwrap().as_str(),
            "10110"
        );
        assert_eq!(BitString::from_bits([true, false, true]).as_str(), "101");
        assert_eq!(BitString::new("102").unwrap_err().value(), "102");
        assert_eq!(bits.bit(0), Some(true));
        assert_eq!(bits.bits().filter(|bit| *bit).count(), 5);
    }

    #[test]
    fn a_range_prints_and_parses_postgres_spellings() {
        let cases = [
            (Range::closed(1, 5), "[1,5]"),
            (Range::open(1, 5), "(1,5)"),
            (Range::closed_open(1, 5), "[1,5)"),
            (Range::open_closed(1, 5), "(1,5]"),
            (Range::new(Bound::Unbounded, Bound::Excluded(5)), "(,5)"),
            (Range::new(Bound::Included(1), Bound::Unbounded), "[1,)"),
            (Range::new(Bound::<i32>::Unbounded, Bound::Unbounded), "(,)"),
        ];
        for (range, text) in cases {
            assert_eq!(range.to_string(), text);
            assert_eq!(text.parse::<Range<i32>>(), Ok(range), "{text}");
        }
        assert_eq!(Range::<i32>::empty().to_string(), "empty");
        assert_eq!("empty".parse::<Range<i32>>(), Ok(Range::empty()));
        assert!("[1,5".parse::<Range<i32>>().is_err());
        assert!("[a,5]".parse::<Range<i32>>().is_err());
    }

    #[test]
    fn a_range_contains_what_its_bounds_say() {
        let range = Range::closed_open(1, 5);
        assert!(range.contains(&1));
        assert!(range.contains(&4));
        assert!(!range.contains(&5));
        assert!(!Range::<i32>::empty().contains(&1));
        assert!(Range::new(Bound::<i32>::Unbounded, Bound::Unbounded).contains(&i32::MIN));
        assert!(!Range::open(1, 1).contains(&1));
    }

    #[test]
    fn a_quoted_range_bound_survives_a_round_trip() {
        let range = Range::new(
            Bound::Included("a,b".to_owned()),
            Bound::Excluded("c".to_owned()),
        );
        // `Display` writes the values bare; the parser accepts both bare and quoted bounds,
        // which is the spelling PostgreSQL itself writes for a text range.
        assert_eq!(range.to_string(), "[a,b,c)");
        assert_eq!(
            "[\"a,b\",\"c\")".parse::<Range<String>>(),
            Ok(range.clone())
        );
    }

    #[test]
    fn a_range_maps_its_bounds() {
        let mapped = Range::closed_open(1, 5).map(|value| value * 2);
        assert_eq!(mapped, Range::closed_open(2, 10));
        assert_eq!(Range::<i32>::empty().map(i64::from), Range::<i64>::empty());
    }

    #[test]
    fn a_geometry_round_trips_its_ewkb_and_mysql_form() {
        // `SRID=4326;POINT(1 2)` in EWKB.
        let ewkb = "0101000020E6100000000000000000F03F0000000000000040";
        let geometry = Geometry::from_hex(ewkb).unwrap();
        assert_eq!(geometry.srid(), Some(4326));
        assert_eq!(geometry.kind(), Some(GeometryKind::Point));
        assert_eq!(geometry.to_hex(), ewkb);
        assert_eq!(geometry.to_string(), ewkb);

        // MySQL keeps the SRID in a four-byte little-endian prefix and the WKB beside it.
        let mysql = geometry.to_mysql_bytes();
        assert_eq!(
            mysql,
            vec![
                0xE6, 0x10, 0x00, 0x00, // 4326, little-endian
                0x01, 0x01, 0x00, 0x00, 0x00, // little-endian Point
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF0, 0x3F, // 1.0
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // 2.0
            ]
        );
        assert_eq!(Geometry::from_mysql_bytes(&mysql).unwrap(), geometry);

        // A geometry with no SRID has a zero prefix and comes back unchanged.
        let plain = Geometry::from_hex("0101000000000000000000F03F0000000000000040").unwrap();
        assert_eq!(plain.srid(), None);
        let mysql = plain.to_mysql_bytes();
        assert_eq!(&mysql[..4], &[0, 0, 0, 0]);
        assert_eq!(Geometry::from_mysql_bytes(&mysql).unwrap(), plain);

        assert!(Geometry::from_hex("not hex").is_err());
        assert!(Geometry::from_hex("0101").is_err());
    }

    #[test]
    fn a_geometry_of_an_unnamed_shape_still_round_trips() {
        // Type 8 is a circular string: Besu has no name for it, but the bytes are still a
        // value it can store and return unchanged.
        let geometry = Geometry::from_hex("0108000020E6100000").unwrap();
        assert_eq!(geometry.kind(), None);
        assert_eq!(geometry.srid(), Some(4326));
        assert_eq!(geometry.to_hex(), "0108000020E6100000");

        let mysql = geometry.to_mysql_bytes();
        assert_eq!(&mysql[..4], &[0xE6, 0x10, 0x00, 0x00]);
        assert_eq!(Geometry::from_mysql_bytes(&mysql).unwrap(), geometry);
    }

    #[test]
    fn a_geometry_keeps_a_shape_besu_does_not_name() {
        // A hypothetical type number outside 1..=7 is rejected, but an unnamed shape name is
        // kept verbatim by `GeometryKind`.
        assert_eq!(
            GeometryKind::from_name("PolyhedralSurface").name(),
            "PolyhedralSurface"
        );
        assert_eq!(GeometryKind::from_name("point"), GeometryKind::Point);
        assert_eq!(GeometryKind::from_code(1), GeometryKind::Point);
    }

    #[test]
    fn a_vector_prints_parses_and_round_trips_its_binary_form() {
        let vector = Vector::new(vec![1.0, 2.5, -3.0]);
        assert_eq!(vector.to_string(), "[1,2.5,-3]");
        assert_eq!("[1,2.5,-3]".parse::<Vector>(), Ok(vector.clone()));
        assert_eq!("[ 1 , 2.5 , -3 ]".parse::<Vector>(), Ok(vector.clone()));
        assert_eq!(Vector::default().to_string(), "[]");
        assert_eq!("[]".parse::<Vector>(), Ok(Vector::default()));
        assert!("1,2,3".parse::<Vector>().is_err());

        let binary = vector.to_binary();
        assert_eq!(&binary[..4], &[0, 3, 0, 0]);
        assert_eq!(Vector::from_binary(&binary), Ok(vector));
        assert!(Vector::from_binary(&binary[..binary.len() - 1]).is_err());
        assert!(Vector::from_binary(&[]).is_err());
    }
}
