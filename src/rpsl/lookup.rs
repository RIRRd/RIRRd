//! Lookup fields shared by inverse queries and set resolution.

use std::{borrow::Cow, fmt, net::Ipv6Addr};

use super::{
    MAX_SNAPSHOT_OBJECT_BYTES, Object, asn, dns_name, generic_name, ipv4_address, ipv4_network,
};
use super::{canonical_prefix, is_continuation, prefix_length, set_name};

const INVERSE_FIELDS: &[&str] = &[
    "mnt-by",
    "members",
    "mp-members",
    "member-of",
    "mbrs-by-ref",
    "admin-c",
    "tech-c",
    "zone-c",
    "person",
    "role",
];

pub(crate) fn is_inverse_field(field: &str) -> bool {
    INVERSE_FIELDS
        .iter()
        .any(|name| name.eq_ignore_ascii_case(field))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LookupAttributes {
    pub mnt_by: Vec<String>,
    pub member_of: Vec<String>,
    pub mbrs_by_ref: Vec<String>,
    pub members: Vec<SetMember>,
    pub mp_members: Vec<SetMember>,
    pub admin_c: Vec<String>,
    pub tech_c: Vec<String>,
    pub zone_c: Vec<String>,
    pub person: Vec<String>,
    pub role: Vec<String>,
    pub skipped_invalid_values: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetMember {
    Asn(u32, Option<RangeOperator>),
    Set(String, Option<RangeOperator>),
    Prefix(String, Option<RangeOperator>),
    Router(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOperator {
    MoreSpecific,
    MoreSpecificOrEqual,
    Length(u16),
    Interval(u16, u16),
}

impl fmt::Display for SetMember {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let range = match self {
            Self::Asn(number, range) => {
                write!(formatter, "AS{number}")?;
                range
            }
            Self::Set(name, range) | Self::Prefix(name, range) => {
                formatter.write_str(name)?;
                range
            }
            Self::Router(name) => return formatter.write_str(name),
        };
        match range {
            Some(RangeOperator::MoreSpecific) => formatter.write_str("^-"),
            Some(RangeOperator::MoreSpecificOrEqual) => formatter.write_str("^+"),
            Some(RangeOperator::Length(length)) => write!(formatter, "^{length}"),
            Some(RangeOperator::Interval(start, end)) => write!(formatter, "^{start}-{end}"),
            None => Ok(()),
        }
    }
}

impl LookupAttributes {
    /// Canonical index entries. Repeated values can be deduplicated by callers.
    pub fn inverse_entries(&self) -> impl Iterator<Item = (&'static str, Cow<'_, str>)> {
        [
            ("mnt-by", self.mnt_by.as_slice()),
            ("member-of", self.member_of.as_slice()),
            ("mbrs-by-ref", self.mbrs_by_ref.as_slice()),
            ("admin-c", self.admin_c.as_slice()),
            ("tech-c", self.tech_c.as_slice()),
            ("zone-c", self.zone_c.as_slice()),
            ("person", self.person.as_slice()),
            ("role", self.role.as_slice()),
        ]
        .into_iter()
        .flat_map(|(field, values)| {
            values
                .iter()
                .map(move |value| (field, value.as_str().into()))
        })
        .chain(
            [("members", &self.members), ("mp-members", &self.mp_members)]
                .into_iter()
                .flat_map(|(field, values)| {
                    values
                        .iter()
                        .map(move |value| (field, Cow::Owned(value.to_string().to_uppercase())))
                }),
        )
    }
}

impl Object {
    /// Extract indexed attributes from one bounded, already parsed object.
    ///
    /// Unknown attributes are ignored here and retained in the original text.
    /// Invalid lookup values are counted and skipped. An invalid token in a
    /// `mbrs-by-ref` occurrence suppresses that whole occurrence.
    pub fn lookup_attributes(&self) -> Result<LookupAttributes, String> {
        lookup_attributes(&self.class, &self.text)
    }
}

/// Extract lookup fields from borrowed storage text for an already parsed object.
pub(crate) fn lookup_attributes(class: &str, text: &str) -> Result<LookupAttributes, String> {
    if text.len() > MAX_SNAPSHOT_OBJECT_BYTES {
        return Err("object exceeds lookup extraction limit".into());
    }
    let mut result = LookupAttributes::default();
    let mut field = None;
    let mut value = Cow::Borrowed("");
    for line in text.lines() {
        if line.starts_with(['#', '%']) {
            continue;
        }
        if is_continuation(line) {
            if field.is_some() {
                let value = value.to_mut();
                value.push('\n');
                value.push_str(&line[1..]);
            }
            continue;
        }
        if let Some(field) = field.take() {
            result.insert(class, field, &value);
        }
        if let Some((name, raw)) = line.split_once(':') {
            field = lookup_field(class, name);
            value = Cow::Borrowed(raw);
        }
    }
    if let Some(field) = field {
        result.insert(class, field, &value);
    }
    Ok(result)
}

fn lookup_field(class: &str, name: &str) -> Option<&'static str> {
    let field = INVERSE_FIELDS
        .iter()
        .copied()
        .find(|field| name.eq_ignore_ascii_case(field))?;
    let supported = match field {
        "members" | "mbrs-by-ref" => matches!(class, "as-set" | "route-set" | "rtr-set"),
        "mp-members" => matches!(class, "route-set" | "rtr-set"),
        "member-of" => matches!(class, "aut-num" | "route" | "route6" | "inet-rtr"),
        "admin-c" | "tech-c" => class != "person",
        "zone-c" => class == "domain",
        "person" | "role" => class == field,
        _ => true,
    };
    supported.then_some(field)
}

impl LookupAttributes {
    fn insert(&mut self, class: &str, field: &str, raw: &str) {
        let value = super::normalise_attribute(raw);
        let list = || {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
        };
        match field {
            "members" | "mp-members" => {
                let destination = if field == "members" {
                    &mut self.members
                } else {
                    &mut self.mp_members
                };
                for member in list() {
                    if let Some(member) = parse_member(class, field == "mp-members", member) {
                        destination.push(member);
                    } else {
                        self.skipped_invalid_values += 1;
                    }
                }
            }
            "member-of" => {
                for name in list() {
                    if let Ok(name) = set_name(name) {
                        self.member_of.push(name);
                    } else {
                        self.skipped_invalid_values += 1;
                    }
                }
            }
            "mnt-by" | "mbrs-by-ref" => {
                let destination = if field == "mnt-by" {
                    &mut self.mnt_by
                } else {
                    &mut self.mbrs_by_ref
                };
                let previous_len = destination.len();
                let previous_invalid_count = self.skipped_invalid_values;
                for maintainer in list() {
                    if generic_name(maintainer) {
                        destination.push(maintainer.to_ascii_uppercase());
                    } else {
                        self.skipped_invalid_values += 1;
                    }
                }
                if field == "mbrs-by-ref" && self.skipped_invalid_values != previous_invalid_count {
                    self.skipped_invalid_values += destination.len() - previous_len;
                    destination.truncate(previous_len);
                }
            }
            "admin-c" => push_text(&mut self.admin_c, &value),
            "tech-c" => push_text(&mut self.tech_c, &value),
            "zone-c" => push_text(&mut self.zone_c, &value),
            "person" => push_text(&mut self.person, &value),
            "role" => push_text(&mut self.role, &value),
            _ => unreachable!("lookup_field returns a known field"),
        }
    }
}

fn push_text(destination: &mut Vec<String>, value: &str) {
    if !value.is_empty() {
        destination.push(value.to_uppercase());
    }
}

fn parse_member(class: &str, multiprotocol: bool, member: &str) -> Option<SetMember> {
    if class == "rtr-set" {
        let name = member.to_ascii_uppercase();
        if name.split(':').any(|part| part.starts_with("RTRS-")) {
            return Some(SetMember::Set(set_name(&name).ok()?, None));
        }
        return if dns_name(member) {
            Some(SetMember::Router(name))
        } else {
            Some(SetMember::Set(set_name(&name).ok()?, None))
        };
    }
    let (member, range) = if class == "route-set" {
        if let Some((member, range)) = member.split_once('^') {
            (member, Some(parse_range(range)?))
        } else {
            (member, None)
        }
    } else {
        (member, None)
    };
    if let Ok(number) = asn(member) {
        return Some(SetMember::Asn(number, range));
    }
    if let Ok(name) = set_name(member) {
        return Some(SetMember::Set(name, range));
    }
    if class == "route-set" {
        return parse_prefix_member(member, multiprotocol, range);
    }
    None
}

pub(crate) fn parse_range(value: &str) -> Option<RangeOperator> {
    let length = |value: &str| {
        if value.len() > 3 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        value.parse::<u16>().ok()
    };
    match value {
        "-" => Some(RangeOperator::MoreSpecific),
        "+" => Some(RangeOperator::MoreSpecificOrEqual),
        _ => match value.split_once('-') {
            Some((start, end)) => {
                let start = length(start)?;
                let end = length(end)?;
                (start <= end).then_some(RangeOperator::Interval(start, end))
            }
            None => Some(RangeOperator::Length(length(value)?)),
        },
    }
}

fn parse_prefix_member(
    member: &str,
    multiprotocol: bool,
    range: Option<RangeOperator>,
) -> Option<SetMember> {
    let prefix = if member.contains(':') {
        if !multiprotocol {
            return None;
        }
        if member.contains('/') {
            canonical_prefix(member, true).ok()?
        } else {
            format!("{}/128", member.parse::<Ipv6Addr>().ok()?)
        }
    } else if let Some((address, length)) = member.split_once('/') {
        let length = prefix_length(length).ok()?;
        let ip = ipv4_network(address, length, false).ok()?;
        format!("{ip}/{length}")
    } else {
        format!("{}/32", ipv4_address(member, false)?)
    };
    let (_, length) = prefix.rsplit_once('/')?;
    let length: u16 = length.parse().ok()?;
    if matches!(range, Some(RangeOperator::Length(start) | RangeOperator::Interval(start, _)) if start < length)
    {
        return None;
    }
    Some(SetMember::Prefix(prefix, range))
}

#[cfg(test)]
mod tests {
    use super::super::{is_inverse_field, lookup_attributes, parse};
    use super::*;

    #[test]
    fn extracts_canonical_members_maintainers_and_contact_names() {
        let object = parse(
            "route-set: rs-test\r\n\
            MNT-BY: mnt-foo, mnt-bar\r\n\
            mnt-by: MNT-BAZ\r\n\
            mbrs-by-ref: any\r\n\
            members: as064496, rs-child^+,\r\n\
            + 192.000.002.0/24^26-28 # example\r\n\
            mp-members: 2001:0DB8::/32^48, 2001:db8::1\r\n\
            admin-c: noc@example.net\r\n\
            source: TEST\r\n",
        )
        .unwrap();
        let lookup = object.lookup_attributes().unwrap();
        assert_eq!(
            lookup,
            lookup_attributes(&object.class, &object.text).unwrap()
        );
        assert!(is_inverse_field("MP-MEMBERS"));
        assert!(!is_inverse_field("auth"));
        assert_eq!(lookup.mnt_by, ["MNT-FOO", "MNT-BAR", "MNT-BAZ"]);
        assert_eq!(lookup.mbrs_by_ref, ["ANY"]);
        assert_eq!(lookup.admin_c, ["NOC@EXAMPLE.NET"]);
        assert_eq!(
            lookup.members,
            [
                SetMember::Asn(64496, None),
                SetMember::Set("RS-CHILD".into(), Some(RangeOperator::MoreSpecificOrEqual)),
                SetMember::Prefix("192.0.2.0/24".into(), Some(RangeOperator::Interval(26, 28))),
            ]
        );
        assert_eq!(
            lookup.mp_members,
            [
                SetMember::Prefix("2001:db8::/32".into(), Some(RangeOperator::Length(48))),
                SetMember::Prefix("2001:db8::1/128".into(), None),
            ]
        );
        assert!(
            lookup
                .inverse_entries()
                .any(|(field, value)| field == "mp-members" && value == "2001:DB8::/32^48")
        );
    }

    #[test]
    fn observes_class_specific_fields_and_keeps_unknown_attributes() {
        let object = parse(
            "aut-num: AS64496\nmember-of: as064497:as-child, AS-ROOT\n\
            mp-members: not relevant\nmbrs-by-ref: not relevant\nsource: TEST\n",
        )
        .unwrap();
        let lookup = object.lookup_attributes().unwrap();
        assert_eq!(lookup.member_of, ["AS64497:AS-CHILD", "AS-ROOT"]);
        assert!(lookup.mp_members.is_empty());
        assert!(lookup.mbrs_by_ref.is_empty());
        assert!(object.text.contains("mp-members: not relevant"));

        let router_set = parse(
            "rtr-set: RTRS-TEST\nmp-members: r1.example.net, as064497:rtrs-other\nsource: TEST\n",
        )
        .unwrap();
        assert_eq!(
            router_set.lookup_attributes().unwrap().mp_members,
            [
                SetMember::Router("R1.EXAMPLE.NET".into()),
                SetMember::Set("AS64497:RTRS-OTHER".into(), None),
            ]
        );
    }

    #[test]
    fn skips_invalid_tokens_and_discards_malformed_membership_grants() {
        let object = parse(
            "route-set: RS-TEST\n\
            members: AS64497,2001:db8::/32,192.0.2.0/24^23,192.0.2.0/24^31-29\n\
            mbrs-by-ref: ANY,broken!\n\
            mbrs-by-ref: MNT-EXAMPLE\n\
            mnt-by: MNT-SAFE,ignored!\nsource: TEST\n",
        )
        .unwrap();
        let lookup = object.lookup_attributes().unwrap();
        assert_eq!(lookup.members, [SetMember::Asn(64497, None)]);
        assert_eq!(lookup.mbrs_by_ref, ["MNT-EXAMPLE"]);
        assert_eq!(lookup.mnt_by, ["MNT-SAFE"]);
        assert_eq!(lookup.skipped_invalid_values, 6);
    }
}
