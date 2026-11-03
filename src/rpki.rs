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
}

impl RoaTable {
    /// Read a complete JSON document exported by a trusted RPKI validator.
    pub fn read(reader: impl Read, max_bytes: u64) -> Result<Self> {
        let limit = max_bytes.saturating_add(1);
        let mut reader = BufReader::new(reader.take(limit));
        let feed = serde_json::from_reader::<_, JsonObject<Feed>>(&mut reader);
        ensure!(
            limit - reader.get_ref().limit() <= max_bytes,
            "ROA feed exceeds the configured byte limit"
        );
        let mut vrps = feed?.0.roas;
        vrps.sort_unstable();
        vrps.dedup();
        Ok(Self { vrps })
    }

    /// Construct the ROV index directly from the embedded validator's verified payloads.
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
        vrps.sort_unstable();
        vrps.dedup();
        Ok(Self { vrps })
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
        let mut covered = false;
        for length in (0..=route.length()).rev() {
            let candidate = route.ancestor(length);
            // The last entry at or before (prefix, origin) is the exact
            // origin's greatest maxLength when such an entry exists.
            let upper = self
                .vrps
                .partition_point(|entry| (entry.prefix, entry.origin) <= (candidate, origin));
            if let Some(entry) = upper.checked_sub(1).and_then(|index| self.vrps.get(index))
                && entry.prefix == candidate
            {
                covered = true;
                if origin != 0 && origin == entry.origin && route.length() <= entry.max_length {
                    return Ok(Validation::Valid);
                }
            } else if self
                .vrps
                .get(upper)
                .is_some_and(|entry| entry.prefix == candidate)
            {
                covered = true;
            }
        }
        Ok(if covered {
            Validation::Invalid
        } else {
            Validation::NotFound
        })
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
