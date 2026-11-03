//! Structural RPSL parsing for mirror snapshots.
//!
//! Recognises the RFC 2622/4012 record structure and validates source and object
//! identity. Optional attributes are preserved verbatim. Authoritative use
//! requires additional attribute, policy, reference and authorisation checks.

use std::borrow::Cow;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

#[path = "rpsl/lookup.rs"]
pub mod lookup;
pub(crate) use lookup::{is_inverse_field, lookup_attributes};

pub const MAX_OBJECT_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct Object {
    pub source: String,
    pub class: String,
    pub key: String,
    pub text: String,
}

impl fmt::Debug for Object {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Object")
            .field("source", &self.source)
            .field("class", &self.class)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl Object {
    /// Render for unprivileged queries. The stored text can contain credentials.
    pub fn public_text(&self) -> Cow<'_, str> {
        let sensitive = |name: &str| {
            name.eq_ignore_ascii_case("auth") || name.eq_ignore_ascii_case("rpki-ov-state")
        };
        let needs_filtering = self.text.lines().any(|line| {
            line.starts_with(['#', '%'])
                || line
                    .split_once(':')
                    .is_some_and(|(name, _)| sensitive(name))
        });
        if !needs_filtering {
            return Cow::Borrowed(&self.text);
        }

        let mut result = String::with_capacity(self.text.len());
        let mut suppress_continuations = false;
        for segment in self.text.split_inclusive('\n') {
            let line = segment.trim_end_matches(['\r', '\n']);
            if line.starts_with(['#', '%']) {
                continue;
            }
            if !is_continuation(line) {
                suppress_continuations = line
                    .split_once(':')
                    .is_some_and(|(name, _)| sensitive(name));
            }
            if !suppress_continuations {
                result.push_str(segment);
            }
        }
        Cow::Owned(result)
    }
}

#[derive(Default)]
struct Identity {
    class: String,
    first: Option<String>,
    source: Option<String>,
    second: Option<String>,
}

/// Parse one UTF-8 object, keeping the input text and canonicalising its key.
pub fn parse(text: &str) -> Result<Object, String> {
    if text.len() > MAX_OBJECT_BYTES {
        return Err(format!("RPSL object exceeds {MAX_OBJECT_BYTES} bytes"));
    }

    let mut identity = Identity::default();
    let mut current: Option<&mut Option<String>> = None;
    let mut separator = false;
    let mut has_attribute = false;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if line
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(format!("line {line_no}: invalid control character"));
        }
        if line.starts_with(['#', '%']) {
            continue;
        }
        if line.is_empty() {
            separator = has_attribute;
            continue;
        }
        if separator {
            return Err(format!("line {line_no}: multiple objects in one record"));
        }
        if is_continuation(line) {
            if !has_attribute {
                return Err(format!("line {line_no}: continuation without an attribute"));
            }
            if let Some(value) = current.as_deref_mut().and_then(Option::as_mut) {
                value.push('\n');
                value.push_str(&line[1..]);
            }
            continue;
        }

        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("line {line_no}: expected an attribute"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(format!("line {line_no}: invalid attribute name"));
        }
        let name = name.to_ascii_lowercase();
        if matches!(name.as_str(), "password" | "override" | "api-key") {
            return Err(format!(
                "line {line_no}: submission credentials in snapshot object"
            ));
        }

        if !has_attribute {
            if !supported_class(&name) {
                return Err(format!("line {line_no}: unsupported object class"));
            }
            identity.class = name.clone();
            has_attribute = true;
        }

        let secondary_name = match identity.class.as_str() {
            "route" | "route6" => "origin",
            "person" | "role" => "nic-hdl",
            _ => "",
        };
        current = match name.as_str() {
            "source" => Some(&mut identity.source),
            _ if name == identity.class => Some(&mut identity.first),
            _ if name == secondary_name => Some(&mut identity.second),
            _ => None,
        };
        if let Some(slot) = current.as_deref_mut() {
            if slot.is_some() {
                return Err(format!("line {line_no}: duplicate {name} attribute"));
            }
            *slot = Some(value.to_owned());
        }
    }

    let first = required(identity.first.as_deref(), "class identity")?;
    let source = required(identity.source.as_deref(), "source")?;
    if !generic_name(&source) {
        return Err("invalid source name".into());
    }
    let key = match identity.class.as_str() {
        "route" | "route6" => {
            let prefix = canonical_prefix(&first, identity.class == "route6")?;
            let origin = asn(&required(identity.second.as_deref(), "origin")?)?;
            format!("{prefix}AS{origin}").to_ascii_uppercase()
        }
        "aut-num" => format!("AS{}", asn(&first)?),
        "as-block" => as_block(&first)?,
        "inetnum" => ipv4_range(&first)?,
        "inet6num" => canonical_prefix(&first, true)?.to_ascii_uppercase(),
        "person" | "role" => required(identity.second.as_deref(), "nic-hdl")?.to_uppercase(),
        "domain" => first.to_uppercase(),
        "inet-rtr" if dns_name(&first) => first.to_ascii_uppercase(),
        "as-set" | "filter-set" | "peering-set" | "route-set" | "rtr-set" => set_name(&first)?,
        "irt" | "key-cert" | "mntner" | "organisation" if generic_name(&first) => {
            first.to_ascii_uppercase()
        }
        _ => return Err("invalid object key".into()),
    };

    Ok(Object {
        source: source.to_ascii_uppercase(),
        class: identity.class,
        key,
        text: text.to_owned(),
    })
}

fn supported_class(name: &str) -> bool {
    matches!(
        name,
        "as-block"
            | "as-set"
            | "aut-num"
            | "domain"
            | "filter-set"
            | "inet-rtr"
            | "inet6num"
            | "inetnum"
            | "irt"
            | "key-cert"
            | "mntner"
            | "organisation"
            | "peering-set"
            | "person"
            | "role"
            | "route"
            | "route-set"
            | "route6"
            | "rtr-set"
    )
}

fn is_continuation(line: &str) -> bool {
    line.starts_with([' ', '\t', '+'])
}

fn without_comment(value: &str) -> &str {
    value.split_once('#').map_or(value, |(prefix, _)| prefix)
}

fn required<'a>(value: Option<&'a str>, attribute: &str) -> Result<Cow<'a, str>, String> {
    let normalised = normalise_attribute(value.unwrap_or_default());
    if normalised.is_empty() {
        return Err(format!("missing or empty {attribute} attribute"));
    }
    Ok(normalised)
}

fn normalise_attribute(value: &str) -> Cow<'_, str> {
    if value.contains('\n') {
        Cow::Owned(
            value
                .split('\n')
                .map(|line| without_comment(line).trim_matches([',', ' ', '\t']))
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(","),
        )
    } else {
        Cow::Borrowed(without_comment(value).trim())
    }
}

pub(crate) fn generic_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn dns_name(value: &str) -> bool {
    value.split('.').all(|label| {
        let bytes = label.as_bytes();
        bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    })
}

fn set_name(value: &str) -> Result<String, String> {
    let mut key = String::with_capacity(value.len());
    for component in value.split(':') {
        if !key.is_empty() {
            key.push(':');
        }
        if let Ok(number) = asn(component) {
            use std::fmt::Write as _;
            write!(key, "AS{number}").expect("writing to a String cannot fail");
        } else if generic_name(component) {
            key.push_str(&component.to_ascii_uppercase());
        } else {
            return Err("invalid set name component".into());
        }
    }
    Ok(key)
}

fn asn(value: &str) -> Result<u32, String> {
    let digits = value
        .get(..2)
        .filter(|prefix| prefix.eq_ignore_ascii_case("AS"))
        .map(|_| &value[2..])
        .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("invalid AS number")?;
    digits.parse().map_err(|_| "invalid AS number".into())
}

fn as_block(value: &str) -> Result<String, String> {
    let (first, last) = value.split_once('-').ok_or("expected AS number range")?;
    let first = asn(first.trim())?;
    let last = asn(last.trim())?;
    if first > last {
        return Err("reversed AS number range".into());
    }
    Ok(format!("AS{first} - AS{last}"))
}

fn ipv4_range(value: &str) -> Result<String, String> {
    let value = value.replace(',', "");
    let (first, last) = value.split_once('-').unwrap_or((&value, &value));
    let (first_ip, first) = ipv4_range_end(first.trim())?;
    let (last_ip, last) = ipv4_range_end(last.trim())?;
    if first_ip > last_ip {
        return Err("reversed IPv4 range".into());
    }
    if value.contains('-') {
        Ok(format!("{first} - {last}"))
    } else {
        Ok(first)
    }
}

fn ipv4_range_end(value: &str) -> Result<(Ipv4Addr, String), String> {
    if let Some((address, length)) = value.split_once('/') {
        let length = prefix_length(length)?;
        let ip = ipv4_network(address, length, false)?;
        let key = if length == 32 {
            ip.to_string()
        } else {
            format!("{ip}/{length}")
        };
        Ok((ip, key))
    } else {
        let ip = ipv4_address(value, false).ok_or("invalid IPv4 address")?;
        Ok((ip, ip.to_string()))
    }
}

fn ipv4_address(value: &str, require_four_octets: bool) -> Option<Ipv4Addr> {
    let mut octets = [0; 4];
    let mut count = 0;
    for part in value.split('.') {
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *octets.get_mut(count)? = part.parse().ok()?;
        count += 1;
    }
    if require_four_octets && count != 4 {
        return None;
    }
    Some(Ipv4Addr::from(octets))
}

fn ipv4_network(value: &str, length: u8, require_four_octets: bool) -> Result<Ipv4Addr, String> {
    let address = ipv4_address(value, require_four_octets).ok_or("invalid IPv4 address")?;
    let host_bits = 32_u32
        .checked_sub(u32::from(length))
        .ok_or("invalid IPv4 prefix length")?;
    if u32::from(address).trailing_zeros() < host_bits {
        return Err("IPv4 prefix contains host bits".into());
    }
    Ok(address)
}

fn prefix_length(length: &str) -> Result<u8, String> {
    if !length.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid prefix length".into());
    }
    length.parse().map_err(|_| "invalid prefix length".into())
}

/// Validate and canonicalise a network prefix for the specified address family.
pub fn canonical_prefix(value: &str, ipv6: bool) -> Result<String, String> {
    let (address, length) = value.split_once('/').ok_or("expected CIDR prefix")?;
    let length = prefix_length(length)?;
    if ipv6 {
        let address: Ipv6Addr = address.parse().map_err(|_| "invalid IPv6 address")?;
        let host_bits = 128_u32
            .checked_sub(u32::from(length))
            .ok_or("invalid IPv6 prefix length")?;
        if u128::from(address).trailing_zeros() < host_bits {
            return Err("IPv6 prefix contains host bits".into());
        }
        Ok(format!("{address}/{length}"))
    } else {
        let address = ipv4_network(address, length, true)?;
        Ok(format!("{address}/{length}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_original_text_and_canonicalises_identity() {
        let text = "# example\r\nroute6: 2001:0db8:0000::/32\r\nremarks: multiple\r\n+\r\n\tlines\r\norigin: as064496 # example\r\nsource: test\r\n";
        let object = parse(text).unwrap();
        assert_eq!(object.class, "route6");
        assert_eq!(object.key, "2001:DB8::/32AS64496");
        assert_eq!(object.source, "TEST");
        assert_eq!(object.text, text);
    }

    #[test]
    fn validates_class_specific_identities() {
        for (text, expected) in [
            ("route: 0.0.0.0/0\norigin: AS0", "0.0.0.0/0AS0"),
            ("person: Example Operator\nnic-hdl: eO1-test", "EO1-TEST"),
            ("as-block: as064496-as64511", "AS64496 - AS64511"),
            (
                "inetnum: 192.0.2.0-\n+ 192.0.2.255",
                "192.0.2.0 - 192.0.2.255",
            ),
            ("as-set: as064496:as-example", "AS64496:AS-EXAMPLE"),
        ] {
            assert_eq!(
                parse(&format!("{text}\nsource: TEST")).unwrap().key,
                expected
            );
        }
    }

    #[test]
    fn matches_mirror_key_normalisation_cases_from_irrd_4_5_4() {
        for (text, expected) in [
            (
                "route: 198.051.0100.000/024\norigin: AS64497",
                "198.51.100.0/24AS64497",
            ),
            (
                "inetnum: 198.051.100.0, # fold\n+ -,\n+ 198.51.100.199",
                "198.51.100.0 - 198.51.100.199",
            ),
            ("inetnum: 198.51.100/24", "198.51.100.0/24"),
            ("inetnum: 198.51.100.29/32", "198.51.100.29"),
            (
                "person: Example Operator\nnic-hdl: noc@example.net",
                "NOC@EXAMPLE.NET",
            ),
            ("role: Example Role\nnic-hdl: AB.1-EXAMPLE", "AB.1-EXAMPLE"),
            (
                "domain: 0/25.100.51.198.in-addr.arpa",
                "0/25.100.51.198.IN-ADDR.ARPA",
            ),
            ("inet-rtr: r1.core-example.net", "R1.CORE-EXAMPLE.NET"),
        ] {
            let input = format!("{text}\nsource: TEST");
            let object = parse(&input).unwrap();
            assert_eq!(object.key, expected);
            assert_eq!(object.text, input);
        }
    }

    #[test]
    fn rejects_ambiguous_and_malformed_records() {
        for text in [
            "route: 192.0.2.1/24\norigin: AS64496\nsource: TEST",
            "route6: 2001:db8::/129\norigin: AS64496\nsource: TEST",
            "route: 192.0.2.0/24\norigin: AS4294967296\nsource: TEST",
            "route: 192.0.2.0/24\nsource: TEST",
            "route: 192.0.2.0/+24\norigin: AS64496\nsource: TEST",
            "route: 192.0.2/24\norigin: AS64496\nsource: TEST",
            "inet-rtr: example..net\nsource: TEST",
            "inet-rtr: _r1.example.net\nsource: TEST",
            "mntner: TEST.MNT\nsource: TEST",
            "aut-num: AS64496\nsource: TEST\nSOURCE: OTHER",
            "aut-num: AS64496\nsource: TEST\n\naut-num: AS64497\nsource: TEST",
            "aut-num: AS64496\npassword: secret\nsource: TEST",
            "aut-num: AS64496\nsource: T\0EST",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn public_text_filters_authentication_and_claimed_validation() {
        let object = parse("mntner: TEST-MNT\nAuTh: HASH first\n# comment\n+ second\n\tthird\nsource: TEST\nrpki-ov-state: valid\n").unwrap();
        assert_eq!(object.public_text(), "mntner: TEST-MNT\nsource: TEST\n");
        assert!(!format!("{object:?}").contains("HASH"));
    }

    #[test]
    fn generated_ipv6_spellings_share_the_same_identity() {
        // An integer sequence supplies reproducible address bits and ASNs.
        let mut bits = 0x2001_0db8_5a5a_f00d_9abc_0123_dead_beef_u128;
        for length in 0..=128 {
            bits = bits
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let mask = u128::MAX.checked_shl(128 - length).unwrap_or(0);
            let network = Ipv6Addr::from(bits & mask);
            let expanded = network
                .segments()
                .map(|segment| format!("{segment:04X}"))
                .join(":");
            let origin = bits as u32;
            let standard =
                format!("route6: {network}/{length}\norigin: AS{origin}\nsource: TEST\n");
            let variant = format!(
                "Route6: {expanded}/{length}\r\nORIGIN: as{origin:010}\r\nSOURCE: test\r\n"
            );
            let first = parse(&standard).unwrap();
            let second = parse(&variant).unwrap();
            assert_eq!(first.key, second.key);
            assert_eq!(first.class, second.class);
            assert_eq!(first.source, second.source);
            assert_eq!(second.text, variant);
            if length < 128 {
                let host = Ipv6Addr::from((bits & mask) | 1);
                assert!(
                    parse(&format!(
                        "route6: {host}/{length}\norigin: AS{origin}\nsource: TEST\n"
                    ))
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn generated_auth_continuations_keep_public_identity_and_hide_values() {
        for newline in ["\n", "\r\n"] {
            for leading in [" ", "\t", "+"] {
                for auth_name in ["auth", "AUTH", "AuTh"] {
                    let input = [
                        "mntner: TEST-MNT".into(),
                        format!("{auth_name}: secret-first"),
                        "% a snapshot comment".into(),
                        format!("{leading}secret-continuation"),
                        "remarks: public description".into(),
                        "source: TEST".into(),
                        "".into(),
                    ]
                    .join(newline);
                    let object = parse(&input).unwrap();
                    let public = object.public_text();
                    assert!(!public.contains("secret"));
                    assert!(public.contains("public description"));
                    let reparsed = parse(&public).unwrap();
                    assert_eq!(reparsed.source, object.source);
                    assert_eq!(reparsed.class, object.class);
                    assert_eq!(reparsed.key, object.key);
                    assert_eq!(reparsed.public_text(), public);
                }
            }
        }
    }
}
