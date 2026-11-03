//! Authoritative submission checks for the initial contact and maintainer profile.

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::IpAddr;

use anyhow::{Result, anyhow, ensure};
use time::{Date, Month};

use crate::rpsl::{Object, parse};

const MAX_MAINTAINERS: usize = 64;

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
    for attribute in &fields {
        let name = attribute.name.as_str();
        ensure!(
            required.contains(&name) || optional.contains(&name),
            "unrecognised or operator-managed attribute in authoritative object"
        );
        let first = seen.insert(name);
        if !first && matches!(name, "person" | "role" | "mntner" | "nic-hdl" | "source") {
            return Err(anyhow!("duplicate {name} attribute"));
        }
        match name {
            "mnt-by" => {} // Validate the full list after collecting all occurrences.
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

fn extract_maintainers(fields: &[Attribute<'_>], strict: bool) -> Result<Vec<String>> {
    let mut maintainers = Vec::new();
    let mut seen = HashSet::new();
    for attribute in fields.iter().filter(|attribute| attribute.name == "mnt-by") {
        let mut lines = parts(&attribute.raw).peekable();
        ensure!(lines.peek().is_some(), "empty mnt-by attribute");
        while let Some(mut line) = lines.next() {
            if lines.peek().is_some() {
                line = line.strip_suffix(',').unwrap_or(line);
            }
            for value in line.split(',').map(str::trim) {
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
    }
    ensure!(!maintainers.is_empty(), "missing mnt-by attribute");
    Ok(maintainers)
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

        let route = object("route: 192.0.2.0/24\norigin: AS64496\nsource: EXAMPLE\n");
        assert!(validate(&route).is_err());
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
}
