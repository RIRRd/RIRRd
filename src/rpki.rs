//! Route-origin validation against a local feed of already validated ROA payloads.

use std::fmt;
use std::io::{BufReader, Read};
use std::marker::PhantomData;
use std::net::IpAddr;

use anyhow::{Result, anyhow, ensure};
use serde::de::{self, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

const MAX_VRPS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Validation {
    Valid,
    Invalid,
    NotFound,
}

pub struct RoaTable {
    vrps: Vec<Vrp>,
    lengths_v4: Vec<u8>,
    lengths_v6: Vec<u8>,
}

impl RoaTable {
    fn indexed(mut vrps: Vec<Vrp>) -> Self {
        vrps.sort_unstable();
        vrps.dedup();
        let mut v4 = [false; 33];
        let mut v6 = [false; 129];
        for entry in &vrps {
            match entry.prefix {
                Prefix::V4(_, length) => v4[usize::from(length)] = true,
                Prefix::V6(_, length) => v6[usize::from(length)] = true,
            }
        }
        Self {
            vrps,
            lengths_v4: (0..=32).rev().filter(|&n| v4[usize::from(n)]).collect(),
            lengths_v6: (0..=128).rev().filter(|&n| v6[usize::from(n)]).collect(),
        }
    }

    fn covering_lengths(&self, route: Prefix, inclusive: bool) -> impl Iterator<Item = u8> + '_ {
        let lengths = match route {
            Prefix::V4(..) => &self.lengths_v4,
            Prefix::V6(..) => &self.lengths_v6,
        };
        let first =
            lengths.partition_point(|&n| n > route.length() || (!inclusive && n == route.length()));
        lengths[first..].iter().copied()
    }
    /// Read a complete JSON document exported by a trusted RPKI validator.
    pub fn read(reader: impl Read, max_bytes: u64) -> Result<Self> {
        let limit = max_bytes.saturating_add(1);
        let mut reader = BufReader::new(reader.take(limit));
        let feed = serde_json::from_reader::<_, JsonObject<Feed>>(&mut reader);
        ensure!(
            limit - reader.get_ref().limit() <= max_bytes,
            "ROA feed exceeds the configured byte limit"
        );
        Ok(Self::indexed(feed?.0.roas))
    }

    /// Construct the ROV index directly from the embedded validator's verified payloads.
    #[cfg(test)]
    pub fn from_payloads(payloads: impl IntoIterator<Item = (String, u32, u8)>) -> Result<Self> {
        let mut vrps = Vec::new();
        for (prefix, origin, max_length) in payloads {
            ensure!(vrps.len() < MAX_VRPS, "ROA payload limit exceeded");
            let prefix = Prefix::parse(&prefix)?;
            ensure!(
                max_length >= prefix.length()
                    && max_length
                        <= if matches!(prefix, Prefix::V6(..)) {
                            128
                        } else {
                            32
                        },
                "invalid payload maximum length"
            );
            vrps.push(Vrp {
                prefix,
                origin,
                max_length,
            });
        }
        Ok(Self::indexed(vrps))
    }

    /// Build directly from verified binary prefixes without allocating and
    /// reparsing a CIDR string for every payload on every refresh.
    pub fn from_verified_prefixes(
        payloads: impl IntoIterator<Item = (IpAddr, u8, u32, u8)>,
    ) -> Result<Self> {
        let input = payloads.into_iter();
        let mut vrps = Vec::with_capacity(input.size_hint().0.min(MAX_VRPS));
        for (address, length, origin, max_length) in input {
            ensure!(vrps.len() < MAX_VRPS, "ROA payload limit exceeded");
            let prefix = Prefix::from_parts(address, length)?;
            ensure!(
                (length..=prefix.width()).contains(&max_length),
                "invalid payload maximum length"
            );
            vrps.push(Vrp {
                prefix,
                origin,
                max_length,
            });
        }
        Ok(Self::indexed(vrps))
    }

    pub fn family_counts(&self) -> (usize, usize) {
        let ipv4 = self
            .vrps
            .partition_point(|v| matches!(v.prefix, Prefix::V4(..)));
        (ipv4, self.vrps.len() - ipv4)
    }

    /// A bounded explanation, ordered most-specific first. The validation
    /// decision still considers every covering VRP, even when this is truncated.
    pub fn covering_payloads(&self, prefix: &str) -> Result<(Vec<serde_json::Value>, bool)> {
        let route = Prefix::parse(prefix)?;
        let mut result = Vec::new();
        for length in self.covering_lengths(route, true) {
            let candidate = route.ancestor(length);
            let lower = self.vrps.partition_point(|v| v.prefix < candidate);
            for vrp in self.vrps[lower..]
                .iter()
                .take_while(|v| v.prefix == candidate)
            {
                if result.len() == 32 {
                    return Ok((result, true));
                }
                let address = match candidate {
                    Prefix::V4(a, _) => IpAddr::V4(a.into()),
                    Prefix::V6(a, _) => IpAddr::V6(a.into()),
                };
                result.push(serde_json::json!({"prefix":format!("{address}/{length}"),"asn":vrp.origin,"max_length":vrp.max_length}));
            }
        }
        Ok((result, false))
    }

    /// Number of distinct validated prefix, origin and max-length tuples.
    pub fn len(&self) -> usize {
        self.vrps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vrps.is_empty()
    }

    pub fn validate(&self, prefix: &str, origin: u32) -> Result<Validation> {
        let route = Prefix::parse(prefix)?;
        if self.is_empty() {
            return Ok(Validation::NotFound);
        }
        // Exact, valid announcements are common. Do not make them walk or
        // search the length directory before the same direct lookup as before.
        let exact = self.covering_verdict(route, origin, route.length());
        if exact == Some(Validation::Valid) {
            return Ok(Validation::Valid);
        }
        let mut covered = exact.is_some();
        for length in self.covering_lengths(route, false) {
            match self.covering_verdict(route.ancestor(length), origin, route.length()) {
                Some(Validation::Valid) => return Ok(Validation::Valid),
                Some(_) => covered = true,
                None => {}
            }
        }
        Ok(if covered {
            Validation::Invalid
        } else {
            Validation::NotFound
        })
    }

    #[inline]
    fn covering_verdict(&self, candidate: Prefix, origin: u32, length: u8) -> Option<Validation> {
        // The last record before (prefix, origin) carries this origin's largest
        // maxLength when present. Other origins still establish coverage.
        let upper = self
            .vrps
            .partition_point(|entry| (entry.prefix, entry.origin) <= (candidate, origin));
        if let Some(entry) = upper.checked_sub(1).and_then(|n| self.vrps.get(n))
            && entry.prefix == candidate
        {
            return Some(
                if origin != 0 && origin == entry.origin && length <= entry.max_length {
                    Validation::Valid
                } else {
                    Validation::Invalid
                },
            );
        }
        self.vrps
            .get(upper)
            .filter(|entry| entry.prefix == candidate)
            .map(|_| Validation::Invalid)
    }
}

#[derive(Deserialize)]
struct Feed {
    #[serde(deserialize_with = "read_vrps")]
    roas: Vec<Vrp>,
}

// A derived struct also accepts positional arrays. Require object syntax for
// both the document and its records so corrupt feed shapes cannot be applied.
struct JsonObject<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for JsonObject<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = JsonObject<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(JsonObject)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

fn read_vrps<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Vrp>, D::Error> {
    struct VrpSequence;

    impl<'de> Visitor<'de> for VrpSequence {
        type Value = Vec<Vrp>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an array of validated ROA payloads")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut vrps = Vec::new();
            while let Some(JsonObject(vrp)) = sequence.next_element()? {
                if vrps.len() == MAX_VRPS {
                    return Err(de::Error::custom("ROA feed exceeds the record limit"));
                }
                vrps.push(vrp);
            }
            Ok(vrps)
        }
    }

    deserializer.deserialize_seq(VrpSequence)
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "VrpInput")]
struct Vrp {
    prefix: Prefix,
    origin: u32,
    max_length: u8,
}

#[derive(Deserialize)]
struct VrpInput {
    prefix: String,
    asn: NumberOrText,
    #[serde(rename = "maxLength")]
    max_length: NumberOrText,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrText {
    Number(u32),
    Text(String),
}

impl NumberOrText {
    fn as_u32(&self, allow_as_prefix: bool) -> Option<u32> {
        match self {
            Self::Number(number) => Some(*number),
            Self::Text(text) => {
                let mut digits = text.trim_ascii();
                if allow_as_prefix
                    && digits
                        .get(..2)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("AS"))
                {
                    digits = &digits[2..];
                }
                if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                digits.parse().ok()
            }
        }
    }
}

impl TryFrom<VrpInput> for Vrp {
    type Error = anyhow::Error;

    fn try_from(input: VrpInput) -> Result<Self> {
        let prefix = Prefix::parse(&input.prefix)?;
        let origin = input
            .asn
            .as_u32(true)
            .ok_or_else(|| anyhow!("invalid VRP ASN"))?;
        let max_length = input
            .max_length
            .as_u32(false)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| anyhow!("invalid VRP maxLength"))?;
        ensure!(
            (prefix.length()..=prefix.width()).contains(&max_length),
            "VRP maxLength is outside the prefix and address family bounds"
        );
        Ok(Self {
            prefix,
            origin,
            max_length,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prefix {
    V4(u32, u8),
    V6(u128, u8),
}

impl Prefix {
    fn parse(text: &str) -> Result<Self> {
        let (address, length) = text
            .split_once('/')
            .ok_or_else(|| anyhow!("expected a CIDR prefix"))?;
        ensure!(
            length.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid prefix length"
        );
        let length = length.parse::<u8>()?;
        let address: IpAddr = address.parse()?;
        Self::from_parts(address, length)
    }

    fn from_parts(address: IpAddr, length: u8) -> Result<Self> {
        let prefix = match address {
            IpAddr::V4(address) => Self::V4(address.into(), length),
            IpAddr::V6(address) => Self::V6(address.into(), length),
        };
        ensure!(
            length <= prefix.width(),
            "prefix length exceeds address family width"
        );
        ensure!(
            prefix == prefix.ancestor(length),
            "prefix has host bits set"
        );
        Ok(prefix)
    }

    fn length(self) -> u8 {
        match self {
            Self::V4(_, length) | Self::V6(_, length) => length,
        }
    }

    fn width(self) -> u8 {
        match self {
            Self::V4(..) => 32,
            Self::V6(..) => 128,
        }
    }

    fn ancestor(self, length: u8) -> Self {
        let host_bits = u32::from(self.width() - length);
        match self {
            Self::V4(address, _) => Self::V4(
                address & u32::MAX.checked_shl(host_bits).unwrap_or(0),
                length,
            ),
            Self::V6(address, _) => Self::V6(
                address & u128::MAX.checked_shl(host_bits).unwrap_or(0),
                length,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(input: &str) -> Result<RoaTable> {
        RoaTable::read(input.as_bytes(), input.len() as u64)
    }

    #[test]
    fn length_index_matches_full_scan_across_both_families_and_as_zero() -> Result<()> {
        let mut records = Vec::new();
        // Include default routes, sparse lengths, multiple origins and AS0.
        for n in 0..256u32 {
            let v4 = Prefix::V4(0x0a00_0000 | (n << 12), 32).ancestor((n % 33) as u8);
            let v6 = Prefix::V6((0x2001_0db8u128 << 96) | (u128::from(n) << 64), 128)
                .ancestor((n % 129) as u8);
            for prefix in [v4, v6] {
                records.push(Vrp {
                    prefix,
                    origin: if n % 7 == 0 { 0 } else { 64496 + n % 3 },
                    max_length: prefix.width(),
                });
            }
        }
        let table = RoaTable::indexed(records);
        for n in 0..512u32 {
            for query in [
                Prefix::V4(0x0a00_0000 | (n << 12), 32).ancestor((n % 33) as u8),
                Prefix::V6((0x2001_0db8u128 << 96) | (u128::from(n) << 64), 128)
                    .ancestor((n % 129) as u8),
            ] {
                let (address, length) = match query {
                    Prefix::V4(a, l) => (IpAddr::V4(a.into()), l),
                    Prefix::V6(a, l) => (IpAddr::V6(a.into()), l),
                };
                for origin in [0, 64496, 64497, 64498, 64500] {
                    let covering: Vec<_> = table
                        .vrps
                        .iter()
                        .filter(|v| {
                            query.width() == v.prefix.width()
                                && query.length() >= v.prefix.length()
                                && query.ancestor(v.prefix.length()) == v.prefix
                        })
                        .collect();
                    let expected = if covering
                        .iter()
                        .any(|v| origin != 0 && origin == v.origin && length <= v.max_length)
                    {
                        Validation::Valid
                    } else if covering.is_empty() {
                        Validation::NotFound
                    } else {
                        Validation::Invalid
                    };
                    assert_eq!(
                        table.validate(&format!("{address}/{length}"), origin)?,
                        expected
                    );
                }
            }
        }
        let v4_only = RoaTable::from_verified_prefixes([("192.0.2.0".parse()?, 24, 64496, 24)])?;
        assert_eq!(
            v4_only.validate("2001:db8::/32", 64496)?,
            Validation::NotFound
        );
        assert!(v4_only.covering_payloads("2001:db8::/32")?.0.is_empty());
        Ok(())
    }

    #[test]
    fn binary_index_matches_text_and_bounds_covering_explanations() -> Result<()> {
        let binary = RoaTable::from_verified_prefixes([
            ("192.0.2.0".parse()?, 24, 64496, 25),
            ("192.0.2.0".parse()?, 24, 64496, 25),
            ("2001:db8::".parse()?, 32, 64497, 48),
        ])?;
        let text = RoaTable::from_payloads([
            ("192.0.2.0/24".into(), 64496, 25),
            ("2001:db8::/32".into(), 64497, 48),
        ])?;
        assert_eq!(binary.len(), 2);
        assert_eq!(binary.family_counts(), (1, 1));
        for (prefix, origin) in [
            ("192.0.2.0/25", 64496),
            ("192.0.2.0/26", 64496),
            ("192.0.2.0/24", 0),
            ("2001:db8:1::/48", 64497),
            ("2001:db8::/32", 64496),
            ("198.51.100.0/24", 64496),
        ] {
            assert_eq!(
                binary.validate(prefix, origin)?,
                text.validate(prefix, origin)?
            );
            assert_eq!(
                binary.covering_payloads(prefix)?,
                text.covering_payloads(prefix)?
            );
        }
        for (address, length, maximum) in [
            ("192.0.2.1", 24, 24),
            ("192.0.2.0", 33, 33),
            ("2001:db8::", 32, 31),
        ] {
            assert!(
                RoaTable::from_verified_prefixes([(address.parse()?, length, 64496, maximum)])
                    .is_err()
            );
        }
        let many = RoaTable::from_verified_prefixes(
            (1..=100).map(|asn| ("192.0.2.0".parse().unwrap(), 24, asn, 24)),
        )?;
        let (covering, truncated) = many.covering_payloads("192.0.2.0/24")?;
        assert_eq!(covering.len(), 32);
        assert!(truncated);
        assert_eq!(many.validate("192.0.2.0/24", 100)?, Validation::Valid);
        Ok(())
    }

    #[test]
    fn any_matching_cover_authorizes_independently_of_feed_order() -> Result<()> {
        let records = [
            r#"{"prefix":"198.51.100.0/24","asn":"AS64496","maxLength":25,"ta":"fixture"}"#,
            r#"{"prefix":"198.51.100.0/24","asn":"64496","maxLength":"27"}"#,
            r#"{"prefix":"198.51.100.0/25","asn":64497,"maxLength":26}"#,
            r#"{"prefix":"198.51.100.0/24","asn":"as64496","maxLength":25}"#,
        ];
        for records in [
            records.into_iter().collect::<Vec<_>>(),
            records.into_iter().rev().collect(),
        ] {
            let input = format!(
                r#"{{"metadata":{{"ignored":true}},"roas":[{}]}}"#,
                records.join(",")
            );
            let roa = table(&input)?;
            assert_eq!(roa.len(), 3);
            assert_eq!(roa.validate("198.51.100.0/25", 64496)?, Validation::Valid);
            assert_eq!(roa.validate("198.51.100.32/27", 64496)?, Validation::Valid);
            assert_eq!(roa.validate("198.51.100.0/28", 64496)?, Validation::Invalid);
            assert_eq!(roa.validate("198.51.100.64/26", 64497)?, Validation::Valid);
            assert_eq!(
                roa.validate("198.51.100.128/25", 64497)?,
                Validation::Invalid
            );
            assert_eq!(roa.validate("198.51.100.0/25", 0)?, Validation::Invalid);
            assert_eq!(roa.validate("198.51.100.0/25", 64500)?, Validation::Invalid);
            assert_eq!(
                roa.validate("198.51.100.0/23", 64496)?,
                Validation::NotFound
            );
        }
        Ok(())
    }

    #[test]
    fn as_zero_covers_but_never_matches_and_families_are_separate() -> Result<()> {
        let roa = table(
            r#"{"roas":[
                {"prefix":"0.0.0.0/0","asn":"AS0","maxLength":32},
                {"prefix":"192.0.2.1/32","asn":64496,"maxLength":32},
                {"prefix":"2001:db8::/32","asn":64497,"maxLength":48},
                {"prefix":"2001:db8:1::/48","asn":0,"maxLength":128}
            ]}"#,
        )?;
        assert_eq!(roa.validate("192.0.2.0/24", 0)?, Validation::Invalid);
        assert_eq!(roa.validate("192.0.2.1/32", 64496)?, Validation::Valid);
        assert_eq!(roa.validate("2001:db8:1::/48", 64497)?, Validation::Valid);
        assert_eq!(roa.validate("2001:db8:1::/64", 0)?, Validation::Invalid);
        assert_eq!(roa.validate("2001:db9::/32", 0)?, Validation::NotFound);
        assert_eq!(
            roa.validate("::ffff:192.0.2.1/128", 64496)?,
            Validation::NotFound
        );
        assert_eq!(roa.validate("::/0", 64497)?, Validation::NotFound);
        Ok(())
    }

    #[test]
    fn rejects_malformed_json_and_records() {
        for input in [
            "{}",
            "[[]]",
            r#"{"roas":[],"roas":[]}"#,
            r#"{"roas":[["192.0.2.0/24",64496,24]]}"#,
            r#"{"roas":[{"prefix":"192.0.2.1/24","asn":64496,"maxLength":24}]}"#,
            r#"{"roas":[{"prefix":"192.0.2.0/24","asn":64496,"maxLength":23}]}"#,
            r#"{"roas":[{"prefix":"192.0.2.0/24","asn":64496,"maxLength":33}]}"#,
            r#"{"roas":[{"prefix":"2001:db8::/32","asn":"AS4294967296","maxLength":48}]}"#,
            r#"{"roas":[{"prefix":"2001:db8::/32","asn":64496,"maxLength":129}]}"#,
            r#"{"roas":[{"prefix":"2001:db8::/32","asn":64496}]}"#,
            r#"{"roas":[]} {"roas":[]}"#,
        ] {
            assert!(table(input).is_err(), "{input}");
        }
    }

    #[test]
    fn enforces_limit_including_whitespace_and_validates_empty_table_queries() -> Result<()> {
        let input = b"{\"roas\":[]}   ";
        assert!(RoaTable::read(&input[..], input.len() as u64 - 1).is_err());
        let roa = RoaTable::read(&input[..], input.len() as u64)?;
        assert!(roa.is_empty());
        assert_eq!(roa.validate("0.0.0.0/0", 0)?, Validation::NotFound);
        assert!(roa.validate("192.0.2.1/24", 64496).is_err());
        assert!(roa.validate("::/+128", 64496).is_err());
        Ok(())
    }
}
