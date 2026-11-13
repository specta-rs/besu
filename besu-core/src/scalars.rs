//! SQL values that are not a Rust primitive: intervals, network addresses and bit strings.
//!
//! These live in `besu-core` because [`Value`](crate::Value) carries them. The
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

use alloc::string::{String, ToString};
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
}
