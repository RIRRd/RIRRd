//! Schema checks for supported authoritative RPSL classes.

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::IpAddr;

use anyhow::{Result, anyhow, ensure};
use time::{Date, Month};

use crate::rpsl::{Object, parse};

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
        "route" => (&["route", "origin", "mnt-by", "source"], ROUTE_OPTIONAL),
        "route6" => (&["route6", "origin", "mnt-by", "source"], ROUTE_OPTIONAL),
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
                "person" | "role" | "mntner" | "nic-hdl" | "source" | "route" | "route6" | "origin"
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
                ensure!(
                    network.bits != 0 || network.length != if ipv6 { 128 } else { 32 },
                    "unspecified host route is prohibited"
                );
                route = Some(network);
            }
            "origin" => ensure!(
                scalar(attribute)
                    .and_then(asn)
                    .is_some_and(|number| number != 0),
                "origin must be a nonzero AS number"
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
                ensure!(
                    values.all(route_set_name),
                    "invalid route-set reference in member-of"
                );
            }
            "mntner" | "nic-hdl" | "source" | "admin-c" | "tech-c" => {
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
        ensure!(
            (1..=width).contains(&length),
            "route prefix length is zero or out of range"
        );
        let network = Self { bits, length };
        ensure!(
            network.bits & !network.mask() == 0,
            "route prefix contains host bits"
        );
        Ok(network)
    }

    fn mask(self) -> u128 {
        u128::MAX << (128 - self.length)
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

fn route_set_name(value: &str) -> bool {
    let mut has_set_name = false;
    for (index, component) in value.split(':').enumerate() {
        if index == 5 {
            return false;
        }
        if asn(component).is_some() {
            continue;
        }
        if !generic_name(component)
            || !component
                .get(..3)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("RS-"))
            || component.eq_ignore_ascii_case("RS-ANY")
        {
            return false;
        }
        has_set_name = true;
    }
    has_set_name
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
            .any(|prefix| upper.starts_with(prefix))
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

        let unsupported = object("aut-num: AS64496\nsource: EXAMPLE\n");
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
}
