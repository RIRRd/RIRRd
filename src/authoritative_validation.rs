//! Schema checks for supported authoritative RPSL classes.

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::IpAddr;

use anyhow::{Result, anyhow, ensure};
use time::{Date, Month};

use crate::rpsl::{
    Object,
    lookup::{RangeOperator, parse_range},
    parse,
};

const MAX_MAINTAINERS: usize = 64;
const ROUTE_OPTIONAL: &[&str] = &[
    "descr",
    "holes",
    "member-of",
    "admin-c",
    "tech-c",
    "remarks",
    "notify",
    "changed",
];
const AS_SET_OPTIONAL: &[&str] = &[
    "descr",
    "members",
    "mbrs-by-ref",
    "admin-c",
    "tech-c",
    "remarks",
    "notify",
    "changed",
];
const ROUTE_SET_OPTIONAL: &[&str] = &[
    "descr",
    "members",
    "mp-members",
    "mbrs-by-ref",
    "admin-c",
    "tech-c",
    "remarks",
    "notify",
    "changed",
];
const AUT_NUM_OPTIONAL: &[&str] = &[
    "descr",
    "member-of",
    // The pinned IRRd schema treats policy expressions as opaque text.
    "import",
    "mp-import",
    "import-via",
    "export",
    "mp-export",
    "export-via",
    "default",
    "mp-default",
    "remarks",
    "notify",
    "changed",
];

struct Attribute<'a> {
    name: String,
    raw: Cow<'a, str>,
}

/// Validate a new or replacement authoritative object.
pub fn validate(object: &Object) -> Result<()> {
    let (required, optional): (&[&str], &[&str]) = match object.class.as_str() {
        "person" => (
            &[
                "person", "address", "phone", "e-mail", "nic-hdl", "mnt-by", "source",
            ],
            &["fax-no", "remarks", "notify", "changed"],
        ),
        "role" => (
            &[
                "role", "address", "phone", "e-mail", "nic-hdl", "mnt-by", "source",
            ],
            &[
                "trouble", "fax-no", "admin-c", "tech-c", "remarks", "notify", "changed",
            ],
        ),
        "mntner" => (
            &["mntner", "admin-c", "upd-to", "auth", "mnt-by", "source"],
            &["descr", "tech-c", "mnt-nfy", "remarks", "notify", "changed"],
        ),
        "organisation" => (
            &[
                "organisation",
                "org-name",
                "country",
                "address",
                "e-mail",
                "mnt-by",
                "source",
            ],
            &[
                "phone", "fax-no", "admin-c", "tech-c", "remarks", "notify", "changed",
            ],
        ),
        "route" => (&["route", "origin", "mnt-by", "source"], ROUTE_OPTIONAL),
        "route6" => (&["route6", "origin", "mnt-by", "source"], ROUTE_OPTIONAL),
        "as-set" => (&["as-set", "mnt-by", "source"], AS_SET_OPTIONAL),
        "route-set" => (&["route-set", "mnt-by", "source"], ROUTE_SET_OPTIONAL),
        "aut-num" => (
            &[
                "aut-num", "as-name", "admin-c", "tech-c", "mnt-by", "source",
            ],
            AUT_NUM_OPTIONAL,
        ),
        _ => {
            return Err(anyhow!(
                "object class is outside the authoritative submission profile"
            ));
        }
    };

    let fields = attributes(object)?;
    ensure!(
        !object
            .text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\r' | '\n' | '\t')),
        "control character in authoritative object"
    );
    let mut seen = HashSet::new();
    let mut route = None;
    let mut holes = Vec::new();
    let ipv6 = object.class == "route6";
    for attribute in &fields {
        let name = attribute.name.as_str();
        ensure!(
            required.contains(&name) || optional.contains(&name),
            "unrecognised or operator-managed attribute in authoritative object"
        );
        let first = seen.insert(name);
        if !first
            && matches!(
                name,
                "person"
                    | "role"
                    | "mntner"
                    | "nic-hdl"
                    | "source"
                    | "route"
                    | "route6"
                    | "origin"
                    | "as-set"
                    | "route-set"
                    | "aut-num"
                    | "as-name"
                    | "organisation"
                    | "org-name"
            )
        {
            return Err(anyhow!("duplicate {name} attribute"));
        }
        match name {
            "mnt-by" => {} // Validate the full list after collecting all occurrences.
            "route" | "route6" => {
                let network = Network::parse(
                    scalar(attribute).ok_or_else(|| anyhow!("invalid {name} prefix"))?,
                    ipv6,
                )?;
                ensure!(network.length != 0, "default route is prohibited");
                ensure!(
                    network.bits != 0 || network.length != if ipv6 { 128 } else { 32 },
                    "unspecified host route is prohibited"
                );
                route = Some(network);
            }
            "origin" | "aut-num" => ensure!(
                scalar(attribute).and_then(asn).is_some_and(assignable_asn),
                "{name} must be an AS number allowed for authoritative objects"
            ),
            "as-name" => ensure!(
                scalar(attribute).is_some_and(|value| valid_name_with_prefix(value, Some("AS-"))),
                "invalid as-name"
            ),
            "holes" => {
                let mut values = list_values(attribute).peekable();
                ensure!(values.peek().is_some(), "empty holes attribute");
                for value in values {
                    holes.push(Network::parse(value, ipv6)?);
                }
            }
            "member-of" => {
                let mut values = list_values(attribute).peekable();
                ensure!(values.peek().is_some(), "empty member-of attribute");
                let prefix = if object.class == "aut-num" {
                    "AS-"
                } else {
                    "RS-"
                };
                ensure!(
                    values.all(|value| set_name(value, &[prefix])),
                    "invalid set reference in member-of"
                );
            }
            "as-set" | "route-set" => {
                let prefix = if name == "as-set" { "AS-" } else { "RS-" };
                ensure!(
                    scalar(attribute).is_some_and(|value| set_name(value, &[prefix])),
                    "invalid {name} name"
                );
            }
            "members" | "mp-members" => {
                let mut values = list_values(attribute).peekable();
                ensure!(values.peek().is_some(), "empty {name} attribute");
                let valid = if object.class == "as-set" {
                    values.all(|value| asn(value).is_some() || set_name(value, &["AS-"]))
                } else {
                    values.all(|value| route_set_member(value, name == "mp-members"))
                };
                ensure!(valid, "invalid {name} attribute");
            }
            "mbrs-by-ref" => {
                if !scalar(attribute).is_some_and(|value| value.eq_ignore_ascii_case("ANY")) {
                    let mut values = list_values(attribute).peekable();
                    ensure!(values.peek().is_some(), "empty mbrs-by-ref attribute");
                    ensure!(values.all(valid_name), "invalid maintainer in mbrs-by-ref");
                }
            }
            "organisation" | "mntner" | "nic-hdl" | "source" | "admin-c" | "tech-c" => {
                ensure!(
                    scalar(attribute).is_some_and(valid_name),
                    "invalid {name} name or reference"
                );
            }
            "e-mail" | "notify" | "upd-to" | "mnt-nfy" => {
                ensure!(
                    scalar(attribute).is_some_and(valid_email),
                    "invalid {name} address"
                );
            }
            "auth" => {
                ensure!(
                    !attribute.raw.contains('\n')
                        && scalar(attribute).is_some_and(valid_bcrypt_auth),
                    "invalid or unsupported auth credential"
                );
            }
            "changed" => {
                ensure!(
                    scalar(attribute).is_some_and(valid_changed),
                    "invalid changed attribute"
                );
            }
            _ => ensure!(
                parts(&attribute.raw).next().is_some(),
                "empty {name} attribute"
            ),
        }
    }

    for &name in required {
        ensure!(seen.contains(name), "missing {name} attribute");
    }
    if let Some(route) = route {
        ensure!(
            holes.iter().all(|hole| route.strictly_contains(*hole)),
            "holes must be more-specific prefixes within the route"
        );
    }
    extract_maintainers(&fields, true)?;
    Ok(())
}

/// Return the parent ASN required when creating a validated AS-SET or ROUTE-SET.
/// Updates to existing names use the ordinary schema rules.
pub fn set_creation_asn(object: &Object) -> Result<u32> {
    ensure!(
        matches!(object.class.as_str(), "as-set" | "route-set"),
        "set creation policy requires an AS-SET or ROUTE-SET"
    );
    let parent = object
        .key
        .split_once(':')
        .and_then(|(first, _)| asn(first))
        .ok_or_else(|| anyhow!("new set names must start with an ASN component"))?;
    ensure!(
        object.key.split(':').filter_map(asn).all(assignable_asn),
        "new set names must not contain reserved AS numbers"
    );
    Ok(parent)
}

/// Extract authorising maintainers from existing or submitted data.
///
/// Every `mnt-by` occurrence must be interpretable. Other attributes may
/// follow legacy schemas when this function is used on existing data.
pub fn maintainers(object: &Object) -> Result<Vec<String>> {
    extract_maintainers(&attributes(object)?, false)
}

/// Extract strong contact handles from existing or submitted data.
pub fn contacts(object: &Object) -> Result<Vec<String>> {
    let fields = attributes(object)?;
    let mut contacts = Vec::new();
    let mut seen = HashSet::new();
    for attribute in fields
        .iter()
        .filter(|attribute| match attribute.name.as_str() {
            "admin-c" | "tech-c" => object.class != "person",
            "zone-c" => object.class == "domain",
            _ => false,
        })
    {
        let value = scalar(attribute).ok_or_else(|| anyhow!("ambiguous contact reference"))?;
        ensure!(
            !value.contains(',')
                && !value
                    .chars()
                    .any(|character| character.is_control() && character != '\t'),
            "ambiguous contact reference"
        );
        let value = value.to_uppercase();
        if seen.insert(value.clone()) {
            contacts.push(value);
        }
    }
    Ok(contacts)
}

fn attributes(object: &Object) -> Result<Vec<Attribute<'_>>> {
    let parsed = parse(&object.text).map_err(anyhow::Error::msg)?;
    ensure!(
        parsed.class == object.class && parsed.source == object.source && parsed.key == object.key,
        "RPSL object identity does not match its text"
    );

    let mut fields: Vec<Attribute<'_>> = Vec::new();
    for line in object.text.lines() {
        if line.is_empty() || line.starts_with(['#', '%']) {
            continue;
        }
        if line.starts_with([' ', '\t', '+']) {
            let attribute = fields
                .last_mut()
                .ok_or_else(|| anyhow!("orphaned RPSL continuation"))?;
            let raw = attribute.raw.to_mut();
            raw.push('\n');
            raw.push_str(&line[1..]);
        } else {
            let (name, raw) = line
                .split_once(':')
                .ok_or_else(|| anyhow!("malformed RPSL attribute"))?;
            fields.push(Attribute {
                name: name.to_ascii_lowercase(),
                raw: Cow::Borrowed(raw),
            });
        }
    }
    Ok(fields)
}

fn parts(value: &str) -> impl Iterator<Item = &str> {
    value
        .split('\n')
        .map(|line| line.split_once('#').map_or(line, |(head, _)| head).trim())
        .filter(|part| !part.is_empty())
}

fn scalar<'a>(attribute: &'a Attribute<'_>) -> Option<&'a str> {
    let mut parts = parts(&attribute.raw);
    let value = parts.next()?;
    parts.next().is_none().then_some(value)
}

fn list_values<'a>(attribute: &'a Attribute<'_>) -> impl Iterator<Item = &'a str> {
    let mut lines = parts(&attribute.raw).peekable();
    std::iter::from_fn(move || {
        let line = lines.next()?;
        let line = if lines.peek().is_some() {
            line.strip_suffix(',').unwrap_or(line)
        } else {
            line
        };
        Some(line.split(',').map(str::trim))
    })
    .flatten()
}

fn extract_maintainers(fields: &[Attribute<'_>], strict: bool) -> Result<Vec<String>> {
    let mut maintainers = Vec::new();
    let mut seen = HashSet::new();
    for attribute in fields.iter().filter(|attribute| attribute.name == "mnt-by") {
        let mut values = list_values(attribute).peekable();
        ensure!(values.peek().is_some(), "empty mnt-by attribute");
        for value in values {
            let valid = if strict {
                valid_name(value)
            } else {
                generic_name(value)
            };
            ensure!(valid, "invalid maintainer reference in mnt-by");
            let value = value.to_ascii_uppercase();
            if seen.insert(value.clone()) {
                ensure!(
                    maintainers.len() < MAX_MAINTAINERS,
                    "too many distinct maintainer references"
                );
                maintainers.push(value);
            }
        }
    }
    ensure!(!maintainers.is_empty(), "missing mnt-by attribute");
    Ok(maintainers)
}

#[derive(Clone, Copy)]
struct Network {
    // IPv4 addresses occupy the most significant 32 bits. The source class
    // fixes the family for both the route and all of its holes.
    bits: u128,
    length: u8,
}

impl Network {
    fn parse(value: &str, ipv6: bool) -> Result<Self> {
        let (address, length) = value
            .split_once('/')
            .ok_or_else(|| anyhow!("expected a route CIDR prefix"))?;
        ensure!(
            length.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid route prefix length"
        );
        let length = length
            .parse::<u8>()
            .map_err(|_| anyhow!("invalid route prefix length"))?;
        let ip: IpAddr = address
            .parse()
            .map_err(|_| anyhow!("invalid route prefix address"))?;
        let (bits, width) = match ip {
            IpAddr::V4(ip) if !ipv6 => (u128::from(u32::from(ip)) << 96, 32),
            IpAddr::V6(ip) if ipv6 && !address.contains('.') => (u128::from(ip), 128),
            _ => return Err(anyhow!("route prefix address family or notation mismatch")),
        };
        ensure!(length <= width, "route prefix length is out of range");
        let network = Self { bits, length };
        ensure!(
            network.bits & !network.mask() == 0,
            "route prefix contains host bits"
        );
        Ok(network)
    }

    fn mask(self) -> u128 {
        u128::MAX
            .checked_shl(u32::from(128 - self.length))
            .unwrap_or(0)
    }

    fn strictly_contains(self, other: Self) -> bool {
        other.length > self.length && other.bits & self.mask() == self.bits
    }
}

fn asn(value: &str) -> Option<u32> {
    let (prefix, digits) = value.split_at_checked(2)?;
    if !prefix.eq_ignore_ascii_case("AS") || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn assignable_asn(number: u32) -> bool {
    // AS_TRANS and the two last ASNs are reserved. Private-use ASNs are allowed.
    !matches!(number, 0 | 23456 | 65535 | u32::MAX)
}

fn set_name(value: &str, prefixes: &[&str]) -> bool {
    let mut has_set_name = false;
    for (index, component) in value.split(':').enumerate() {
        if index == 5 {
            return false;
        }
        if asn(component).is_some() {
            continue;
        }
        if !generic_name(component)
            || !component.get(..3).is_some_and(|prefix| {
                prefixes
                    .iter()
                    .any(|allowed| prefix.eq_ignore_ascii_case(allowed))
            })
            || component.eq_ignore_ascii_case("AS-ANY")
            || component.eq_ignore_ascii_case("RS-ANY")
        {
            return false;
        }
        has_set_name = true;
    }
    has_set_name
}

fn route_set_member(value: &str, multiprotocol: bool) -> bool {
    let (target, range) = match value.split_once('^') {
        Some((target, range)) => {
            let Some(range) = parse_range(range) else {
                return false;
            };
            (target, Some(range))
        }
        None => (value, None),
    };
    if asn(target).is_some() || set_name(target, &["RS-", "AS-"]) {
        // A range on an ASN or nested set needs an explicit expansion policy.
        return range.is_none();
    }
    let ipv6 = target.contains(':');
    if ipv6 && !multiprotocol {
        return false;
    }
    let width = if ipv6 { 128 } else { 32 };
    let length = if target.contains('/') {
        let Ok(network) = Network::parse(target, ipv6) else {
            return false;
        };
        u16::from(network.length)
    } else {
        if target.parse::<IpAddr>().is_err() || (ipv6 && target.contains('.')) {
            return false;
        }
        width
    };
    match range {
        Some(RangeOperator::Length(end)) => (length..=width).contains(&end),
        Some(RangeOperator::Interval(start, end)) => start >= length && end <= width,
        _ => true,
    }
}

fn generic_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_name(value: &str) -> bool {
    valid_name_with_prefix(value, None)
}

fn valid_name_with_prefix(value: &str, allowed_prefix: Option<&str>) -> bool {
    const RESERVED: &[&str] = &[
        "ANY", "AS-ANY", "RS_ANY", "PEERAS", "AND", "OR", "NOT", "ATOMIC", "FROM", "TO", "AT",
        "ACTION", "ACCEPT", "ANNOUNCE", "EXCEPT", "REFINE", "NETWORKS", "INTO", "INBOUND",
        "OUTBOUND",
    ];
    if !generic_name(value) {
        return false;
    }
    let upper = value.to_ascii_uppercase();
    !RESERVED.contains(&upper.as_str())
        && !["AS-", "RS-", "RTRS-", "FLTR-", "PRNG-"]
            .iter()
            .any(|prefix| Some(*prefix) != allowed_prefix && upper.starts_with(prefix))
}

fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if value.len() > 254
        || local.len() > 64
        || !local.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || b"!$%&'*+-/=?^_`{|}~".contains(&byte)
                })
        })
    {
        return false;
    }
    if let Some(literal) = domain
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return literal.parse::<IpAddr>().is_ok()
            || literal
                .get(..5)
                .filter(|prefix| prefix.eq_ignore_ascii_case("IPV6:"))
                .is_some_and(|_| literal[5..].parse::<std::net::Ipv6Addr>().is_ok());
    }
    domain.split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes[0].is_ascii_alphanumeric()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    })
}

fn valid_changed(value: &str) -> bool {
    let mut fields = value.split_ascii_whitespace();
    if !fields.next().is_some_and(valid_email) {
        return false;
    }
    let Some(date) = fields.next() else {
        return true;
    };
    if fields.next().is_some() || date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let year = date[..4].parse::<i32>().ok();
    let month = date[4..6]
        .parse::<u8>()
        .ok()
        .and_then(|month| Month::try_from(month).ok());
    let day = date[6..].parse::<u8>().ok();
    matches!((year, month, day), (Some(year), Some(month), Some(day)) if year > 0 && Date::from_calendar_date(year, month, day).is_ok())
}

fn valid_bcrypt_auth(value: &str) -> bool {
    let mut fields = value.split_ascii_whitespace();
    // The maintainer authentication module validates the bcrypt payload and cost.
    fields
        .next()
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("BCRYPT-PW"))
        && fields.next().is_some()
        && fields.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERSON: &str = concat!(
        "person: Test Operator\n",
        "address: Test address\n",
        "+ Example locality\n",
        "phone: +1 555 0100\n",
        "e-mail: test.operator+irr@example.net\n",
        "nic-hdl: TEST1-EXAMPLE\n",
        "mnt-by: EXAMPLE-MNT,\n",
        "+ SECONDARY-MNT # continued list\n",
        "mnt-by: example-mnt\n",
        "changed: noc@example.net 20240229\n",
        "source: EXAMPLE\n",
    );

    fn object(text: &str) -> Object {
        parse(text).unwrap()
    }

    #[test]
    fn validates_complete_person_role_and_maintainer_profiles() -> Result<()> {
        let person = object(PERSON);
        validate(&person)?;
        assert_eq!(maintainers(&person)?, ["EXAMPLE-MNT", "SECONDARY-MNT"]);

        let role = object(
            "role: Example NOC\naddress: Test address\nphone: +1 555 0100\n\
            e-mail: noc@[IPv6:2001:db8::1]\nnic-hdl: TEST2-EXAMPLE\n\
            tech-c: test1-example\nadmin-c: TEST1-EXAMPLE\n\
            mnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n",
        );
        validate(&role)?;
        assert_eq!(contacts(&role)?, ["TEST1-EXAMPLE"]);

        let maintainer = object(&format!(
            "mntner: EXAMPLE-MNT\nadmin-c: TEST1-EXAMPLE\nupd-to: test@example.net\n\
            auth: BCRYPT-PW $2b$12${}\nmnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n",
            ".".repeat(53)
        ));
        validate(&maintainer)?;
        assert_eq!(contacts(&maintainer)?, ["TEST1-EXAMPLE"]);
        Ok(())
    }

    #[test]
    fn rejects_unknown_fields_missing_fields_and_bad_syntax() {
        for malformed in [
            PERSON.replace("phone: +1 555 0100\n", ""),
            PERSON.replace("phone:", "random-attr:"),
            PERSON.replace("phone:", "last-modified:"),
            PERSON.replace("phone: +1 555 0100", "phone:"),
            PERSON.replace("test.operator+irr", "test..operator"),
            PERSON.replace("20240229", "20230229"),
            PERSON.replace("TEST1-EXAMPLE", "legacy@example.net"),
            PERSON.replace("TEST1-EXAMPLE", "AS-RESERVED"),
            PERSON.replace("example-mnt", "ANY"),
            PERSON.replace("Test address", "Test\u{0085}address"),
        ] {
            assert!(validate(&object(&malformed)).is_err(), "{malformed}");
        }

        let unsupported = object("filter-set: FLTR-EXAMPLE\nsource: EXAMPLE\n");
        assert!(validate(&unsupported).is_err());
    }

    #[test]
    fn rejects_unsupported_auth_without_echoing_the_value() {
        let maintainer = object(
            "mntner: EXAMPLE-MNT\nadmin-c: TEST1-EXAMPLE\nupd-to: test@example.net\n\
            auth: CRYPT-PW private-placeholder\nmnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n",
        );
        let error = validate(&maintainer).unwrap_err();
        assert_eq!(error.to_string(), "invalid or unsupported auth credential");
    }

    #[test]
    fn legacy_reference_extraction_rejects_ambiguous_values_and_forged_identities() -> Result<()> {
        let legacy = object(
            "domain: example.net\nadmin-c: legacy@example.net\nzone-c: TEST3-EXAMPLE\n\
            mnt-by: AS-HISTORIC\nremarks: historical\u{0085}text\nsource: EXAMPLE\n",
        );
        assert_eq!(contacts(&legacy)?, ["LEGACY@EXAMPLE.NET", "TEST3-EXAMPLE"]);
        assert_eq!(maintainers(&legacy)?, ["AS-HISTORIC"]);

        let malformed = object(
            "role: Example NOC\nnic-hdl: TEST2-EXAMPLE\nadmin-c: TEST1-EXAMPLE,TEST3-EXAMPLE\n\
            mnt-by: EXAMPLE-MNT,\nsource: EXAMPLE\n",
        );
        assert!(contacts(&malformed).is_err());
        assert!(maintainers(&malformed).is_err());

        let mut forged = object(PERSON);
        forged.key = "DIFFERENT-EXAMPLE".into();
        assert!(maintainers(&forged).is_err());
        Ok(())
    }

    const ROUTE: &str = concat!(
        "route: 192.0.2.0/24\n",
        "origin: as64496\n",
        "descr: Example route\n",
        "holes: 192.0.2.0/26,\n",
        "+ 192.0.2.128/26\n",
        "member-of: AS64496:rs-example\n",
        "tech-c: TEST1-EXAMPLE\n",
        "mnt-by: EXAMPLE-MNT\n",
        "source: EXAMPLE\n",
    );

    #[test]
    fn validates_route_schema_references_and_contained_holes() -> Result<()> {
        let route = object(ROUTE);
        validate(&route)?;
        assert_eq!(maintainers(&route)?, ["EXAMPLE-MNT"]);
        assert_eq!(contacts(&route)?, ["TEST1-EXAMPLE"]);
        let route6 = object(
            "route6: 2001:0DB8::/32\norigin: AS64497\nholes: 2001:db8:8000::/33\n\
            member-of: RS-PARENT:RS-EXAMPLE\nmnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n",
        );
        validate(&route6)?;
        Ok(())
    }

    #[test]
    fn rejects_incomplete_route_schema_and_unsupported_policy_fields() {
        for malformed in [
            ROUTE.replace("mnt-by: EXAMPLE-MNT\n", ""),
            ROUTE.replace("descr: Example route", "components: {192.0.2.0/24^-}"),
            ROUTE.replace("descr: Example route", "inject: upon static"),
            ROUTE.replace("descr: Example route", "roa-uri: https://example.net/"),
            ROUTE.replace("descr: Example route", "rpki-ov-state: valid"),
            ROUTE.replace("AS64496:rs-example", "AS64496:AS-EXAMPLE"),
            ROUTE.replace("AS64496:rs-example", "RS-ANY"),
            ROUTE.replace("AS64496:rs-example", "RS-A:RS-B:RS-C:RS-D:RS-E:RS-F"),
            ROUTE.replace("TEST1-EXAMPLE", "invalid contact"),
            ROUTE.replace("192.0.2.128/26", "192.0.2.0/24"),
            ROUTE.replace("192.0.2.128/26", "198.51.100.0/26"),
            ROUTE.replace("192.0.2.128/26", "192.0.2.129/26"),
            ROUTE.replace("192.0.2.128/26", "192.00.02.128/26"),
            ROUTE.replace("192.0.2.128/26", "2001:db8::/32"),
        ] {
            assert!(validate(&object(&malformed)).is_err(), "{malformed}");
        }
    }

    #[test]
    fn rejects_non_routable_identities_and_ambiguous_prefix_syntax() {
        for (class, prefix, origin) in [
            ("route", "192.00.02.0/24", "AS64496"),
            ("route", "0.0.0.0/0", "AS64496"),
            ("route6", "::/0", "AS64496"),
            ("route", "0.0.0.0/32", "AS64496"),
            ("route6", "::/128", "AS64496"),
            ("route", "192.0.2.0/24", "AS0"),
            ("route6", "2001:db8::/32", "AS0"),
        ] {
            let text = format!(
                "{class}: {prefix}\norigin: {origin}\nmnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n"
            );
            assert!(validate(&object(&text)).is_err(), "{text}");
        }
        for (class, prefix) in [
            ("route", "192.0.2.0"),
            ("route", "192.0.2.1/24"),
            ("route6", "2001:db8::1/32"),
            ("route6", "2001:db8::/129"),
        ] {
            let text = format!(
                "{class}: {prefix}\norigin: AS64496\nmnt-by: EXAMPLE-MNT\nsource: EXAMPLE\n"
            );
            assert!(parse(&text).is_err(), "{text}");
        }
    }

    const AS_SET: &str = concat!(
        "as-set: AS064496:AS-EXAMPLE\n",
        "members: AS64497,\n",
        "+ AS64498:AS-CHILD\n",
        "mbrs-by-ref: ANY\n",
        "mnt-by: EXAMPLE-MNT\n",
        "source: EXAMPLE\n",
    );

    const ROUTE_SET: &str = concat!(
        "route-set: AS64496:RS-EXAMPLE\n",
        "members: AS64497, AS-CHILD, RS-CHILD, 0.0.0.0/0^0-24, 192.0.2.1\n",
        "mp-members: 2001:db8::/32^48, ::/0^+\n",
        "mbrs-by-ref: FIRST-MNT, SECOND-MNT\n",
        "mnt-by: EXAMPLE-MNT\n",
        "source: EXAMPLE\n",
    );

    #[test]
    fn validates_set_schemas_and_extracts_creation_parent() -> Result<()> {
        for text in [AS_SET, ROUTE_SET] {
            let object = object(text);
            validate(&object)?;
            assert_eq!(set_creation_asn(&object)?, 64496);
        }

        for name in ["AS-EXAMPLE", "AS0:AS-EXAMPLE", "AS-PARENT:AS-EXAMPLE"] {
            let legacy_name = object(&AS_SET.replace("AS064496:AS-EXAMPLE", name));
            validate(&legacy_name)?;
            assert!(set_creation_asn(&legacy_name).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_or_unsupported_set_members_and_grants() {
        for input in [
            AS_SET.replace("AS064496:AS-EXAMPLE", "AS-ANY"),
            AS_SET.replace("AS064496:AS-EXAMPLE", "AS64496:RS-EXAMPLE"),
            AS_SET.replace("AS64498:AS-CHILD", "RS-CHILD"),
            AS_SET.replace("AS64498:AS-CHILD", "192.0.2.0/24"),
            AS_SET.replace("members:", "mp-members:"),
            AS_SET.replace("mbrs-by-ref: ANY", "mbrs-by-ref: ANY, EXAMPLE-MNT"),
            ROUTE_SET.replace("RS-CHILD,", "RS-CHILD^+,"),
            ROUTE_SET.replace("192.0.2.1", "2001:db8::/32"),
            ROUTE_SET.replace("192.0.2.1", "192.00.02.1"),
            ROUTE_SET.replace("2001:db8::/32^48", "2001:db8::/32^31-48"),
            ROUTE_SET.replace("2001:db8::/32^48", "2001:db8::/32^129"),
            ROUTE_SET.replace("2001:db8::/32^48", "2001:db8::1/32^48"),
            ROUTE_SET.replace("FIRST-MNT, SECOND-MNT", "FIRST-MNT,,SECOND-MNT"),
            ROUTE_SET.replace("mnt-by: EXAMPLE-MNT\n", ""),
        ] {
            assert!(validate(&object(&input)).is_err(), "{input}");
        }
    }

    const AUT_NUM: &str = concat!(
        "aut-num: as064496\n",
        "as-name: AS-EXAMPLE\n",
        "descr: Example autonomous system\n",
        "member-of: AS64497:AS-PARENT, AS-SECOND\n",
        "import: from AS64497 accept AS64497\n",
        "import: from AS64498\n",
        "+ accept AS64498\n",
        "mp-export: afi ipv6.unicast to AS64497 announce AS64496\n",
        "admin-c: TEST1-EXAMPLE\n",
        "tech-c: TEST2-EXAMPLE\n",
        "mnt-by: EXAMPLE-MNT\n",
        "source: EXAMPLE\n",
    );

    #[test]
    fn validates_aut_num_names_policies_and_strong_contacts() -> Result<()> {
        let object = object(AUT_NUM);
        assert_eq!(object.key, "AS64496");
        validate(&object)?;
        assert_eq!(contacts(&object)?, ["TEST1-EXAMPLE", "TEST2-EXAMPLE"]);
        assert_eq!(maintainers(&object)?, ["EXAMPLE-MNT"]);
        Ok(())
    }

    #[test]
    fn rejects_incomplete_or_ambiguous_aut_num_objects() {
        for invalid in [
            AUT_NUM.replace("as064496", "AS0"),
            AUT_NUM.replace("as-name: AS-EXAMPLE", "as-name: AS-ANY"),
            AUT_NUM.replace("as-name: AS-EXAMPLE", "as-name: RS-EXAMPLE"),
            AUT_NUM.replace(
                "as-name: AS-EXAMPLE",
                "as-name: AS-EXAMPLE\nas-name: DUPLICATE",
            ),
            AUT_NUM.replace("as-name: AS-EXAMPLE\n", ""),
            AUT_NUM.replace("admin-c: TEST1-EXAMPLE\n", ""),
            AUT_NUM.replace("tech-c: TEST2-EXAMPLE\n", ""),
            AUT_NUM.replace("mnt-by: EXAMPLE-MNT\n", ""),
            AUT_NUM.replace("AS-SECOND", "RS-SECOND"),
            AUT_NUM.replace("AS-SECOND", "AS64498"),
            AUT_NUM.replace("TEST1-EXAMPLE", "TEST1-EXAMPLE,TEST2-EXAMPLE"),
            AUT_NUM.replace("from AS64497 accept AS64497", "# empty expression"),
            AUT_NUM.replace(
                "from AS64497 accept AS64497",
                "from\u{0085}AS64497 accept ANY",
            ),
            AUT_NUM.replace("descr: Example autonomous system", "status: ASSIGNED"),
        ] {
            assert!(validate(&object(&invalid)).is_err(), "{invalid}");
        }
        for identity in ["64496", "AS4294967296", "AS64496.1"] {
            assert!(parse(&AUT_NUM.replace("as064496", identity)).is_err());
        }
    }

    #[test]
    fn applies_reserved_asn_policy_to_authoritative_identities() -> Result<()> {
        for asn in [0, 23456, 65535, u32::MAX, 64512, 4_200_000_000] {
            let allowed = asn == 64512 || asn == 4_200_000_000;
            let aut_num = object(&AUT_NUM.replace("as064496", &format!("AS{asn}")));
            assert_eq!(validate(&aut_num).is_ok(), allowed);
            for (class, prefix) in [("route", "192.0.2.0/24"), ("route6", "2001:db8::/32")] {
                let route = object(&format!(
                    "{class}: {prefix}\norigin: AS{asn}\nmnt-by: EXAMPLE-MNT\nsource: TEST\n"
                ));
                assert_eq!(validate(&route).is_ok(), allowed);
            }
            let set = object(&AS_SET.replace(
                "AS064496:AS-EXAMPLE",
                &format!("AS64496:AS{asn}:AS-EXAMPLE"),
            ));
            validate(&set)?;
            assert_eq!(set_creation_asn(&set).is_ok(), allowed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod organisation_tests {
    use super::*;

    #[test]
    fn matches_pinned_organisation_fields_and_multiplicity() -> Result<()> {
        let text = "organisation: ORG-TEST\norg-name: Example Organisation\ncountry: GB\naddress: Example Road\ne-mail: noc@example.net\nmnt-by: TEST-MNT\nsource: TEST\n";
        validate(&parse(text).map_err(anyhow::Error::msg)?)?;
        for missing in [
            "org-name: Example Organisation\n",
            "country: GB\n",
            "address: Example Road\n",
            "e-mail: noc@example.net\n",
        ] {
            assert!(
                validate(&parse(&text.replace(missing, "")).map_err(anyhow::Error::msg)?).is_err()
            );
        }
        for extra in [
            "org-name: Duplicate\n",
            "auth: secret\n",
            "e-mail: invalid\n",
            "admin-c: TC1-TEST, TC2-TEST\n",
        ] {
            assert!(
                validate(&parse(&format!("{text}{extra}")).map_err(anyhow::Error::msg)?).is_err()
            );
        }
        validate(
            &parse(&format!(
                "{text}country: NL\naddress: Second line\ne-mail: second@example.net\n"
            ))
            .map_err(anyhow::Error::msg)?,
        )?;
        Ok(())
    }
}
