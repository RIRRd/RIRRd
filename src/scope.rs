//! Scope exclusion predicates for configured route submission policy.

use std::net::IpAddr;

use anyhow::{Context, Result, ensure};

const MAX_FILTERS: usize = 4096;
const MAX_FILTER_BYTES: usize = 128;

#[derive(Clone, Debug)]
pub struct Scope {
    ipv4: Vec<Interval>,
    ipv6: Vec<Interval>,
    asns: Vec<Interval>,
}

impl Scope {
    /// Compile excluded prefixes and inclusive decimal ASN ranges.
    pub fn compile(prefixes: &[String], asns: &[String]) -> Result<Self> {
        ensure!(
            prefixes.len().saturating_add(asns.len()) <= MAX_FILTERS,
            "scope filter count exceeds {MAX_FILTERS}"
        );
        let mut ipv4 = Vec::new();
        let mut ipv6 = Vec::new();
        for (index, prefix) in prefixes.iter().enumerate() {
            let (is_ipv6, interval) = prefix_interval(prefix, true)
                .with_context(|| format!("invalid scope prefix filter {}", index + 1))?;
            if is_ipv6 {
                ipv6.push(interval);
            } else {
                ipv4.push(interval);
            }
        }
        let asns = asns
            .iter()
            .enumerate()
            .map(|(index, asn)| {
                asn_interval(asn).with_context(|| format!("invalid scope ASN filter {}", index + 1))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            ipv4: merge(ipv4),
            ipv6: merge(ipv6),
            asns: merge(asns),
        })
    }

    /// A route is excluded by either its origin or any overlapping network
    /// in the same address family.
    pub fn rejects_route(&self, prefix: &str, origin: u32) -> Result<bool> {
        let (is_ipv6, route) = prefix_interval(prefix, false)?;
        let prefixes = if is_ipv6 { &self.ipv6 } else { &self.ipv4 };
        let origin = u128::from(origin);
        Ok(overlaps(prefixes, route) || overlaps(&self.asns, Interval::new(origin, origin)))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Interval {
    start: u128,
    end: u128,
}

impl Interval {
    fn new(start: u128, end: u128) -> Self {
        Self { start, end }
    }
}

fn merge(mut ranges: Vec<Interval>) -> Vec<Interval> {
    ranges.sort_unstable();
    let mut merged: Vec<Interval> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end.saturating_add(1)
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn overlaps(ranges: &[Interval], query: Interval) -> bool {
    let end = ranges.partition_point(|range| range.start <= query.end);
    end.checked_sub(1)
        .is_some_and(|index| ranges[index].end >= query.start)
}

fn prefix_interval(value: &str, allow_host: bool) -> Result<(bool, Interval)> {
    ensure!(value.len() <= MAX_FILTER_BYTES, "scope prefix is too long");
    let value = value.trim();
    let (address, length) = value
        .split_once('/')
        .map_or((value, None), |(address, length)| (address, Some(length)));
    let address: IpAddr = address.parse().context("invalid IP address")?;
    let (is_ipv6, bits, width) = match address {
        IpAddr::V4(address) => (false, u128::from(u32::from(address)), 32),
        IpAddr::V6(address) => (true, u128::from(address), 128),
    };
    let length = if let Some(length) = length {
        ensure!(
            length.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid prefix length"
        );
        length.parse::<u8>().context("invalid prefix length")?
    } else {
        ensure!(allow_host, "expected a CIDR prefix");
        width
    };
    ensure!(
        length <= width,
        "prefix length exceeds address family width"
    );
    let host_bits = u32::from(width - length);
    let host_mask = u128::MAX.checked_shr(128 - host_bits).unwrap_or(0);
    ensure!(bits & host_mask == 0, "prefix has host bits set");
    Ok((is_ipv6, Interval::new(bits, bits | host_mask)))
}

fn asn_interval(value: &str) -> Result<Interval> {
    ensure!(
        value.len() <= MAX_FILTER_BYTES,
        "scope ASN range is too long"
    );
    let (start, end) = value.split_once('-').unwrap_or((value, value));
    let start = decimal_asn(start.trim())?;
    let end = decimal_asn(end.trim())?;
    ensure!(start <= end, "scope ASN range is reversed");
    Ok(Interval::new(u128::from(start), u128::from(end)))
}

fn decimal_asn(value: &str) -> Result<u32> {
    ensure!(
        value.bytes().all(|byte| byte.is_ascii_digit()),
        "ASN filter must use unsigned decimal notation"
    );
    value
        .parse()
        .context("ASN filter is outside the 32-bit range")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(prefixes: &[&str], asns: &[&str]) -> Result<Scope> {
        Scope::compile(
            &prefixes
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            &asns
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn excludes_exact_more_specific_and_covering_routes() -> Result<()> {
        let scope = scope(&["192.0.2.64/26", "2001:db8:1234::/48"], &[])?;
        for route in [
            "192.0.2.64/26",
            "192.0.2.96/27",
            "192.0.2.0/24",
            "2001:db8:1234::/48",
            "2001:db8:1234:1::/64",
            "2001:db8::/32",
        ] {
            assert!(scope.rejects_route(route, 64496)?, "{route}");
        }
        for route in [
            "192.0.2.0/26",
            "192.0.2.128/25",
            "2001:db8:1235::/48",
            "198.51.100.0/24",
        ] {
            assert!(!scope.rejects_route(route, 64496)?, "{route}");
        }
        Ok(())
    }

    #[test]
    fn separates_families_and_handles_host_and_default_filters() -> Result<()> {
        let scope = scope(&["::/0", "192.0.2.17"], &[])?;
        assert!(scope.rejects_route("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128", 1)?);
        assert!(scope.rejects_route("192.0.2.0/24", 1)?);
        assert!(scope.rejects_route("192.0.2.17/32", 1)?);
        assert!(!scope.rejects_route("192.0.2.18/32", 1)?);
        let scope = Scope::compile(&["0.0.0.0/0".into()], &[])?;
        assert!(!scope.rejects_route("::ffff:192.0.2.17/128", 1)?);
        assert!(scope.rejects_route("255.255.255.255/32", 1)?);
        Ok(())
    }

    #[test]
    fn merges_unsorted_inclusive_ranges_and_preserves_gaps() -> Result<()> {
        let scope = scope(
            &[],
            &[
                "64499-64505",
                "64510",
                "64496-64498",
                "64502-64504",
                "4294967294-4294967295",
            ],
        )?;
        assert_eq!(
            scope.asns,
            [
                Interval::new(64496, 64505),
                Interval::new(64510, 64510),
                Interval::new(u128::from(u32::MAX - 1), u128::from(u32::MAX)),
            ]
        );
        for origin in [64496, 64498, 64499, 64505, 64510, u32::MAX] {
            assert!(scope.rejects_route("2001:db8::/32", origin)?);
        }
        for origin in [64495, 64506, 64509, 64511, u32::MAX - 2] {
            assert!(!scope.rejects_route("2001:db8::/32", origin)?);
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_configuration_and_invalid_query_prefixes() -> Result<()> {
        for prefix in [
            "192.0.2.1/24",
            "192.0.2.0/33",
            "2001:db8::/129",
            "192.0.2.0/+24",
            "",
        ] {
            assert!(scope(&[prefix], &[]).is_err(), "{prefix}");
        }
        for range in [
            "AS64496",
            "64499-64496",
            "64496-4294967296",
            "-1",
            "1-2-3",
            "",
        ] {
            assert!(scope(&[], &[range]).is_err(), "{range}");
        }
        assert!(
            Scope::compile(&vec!["192.0.2.0/24".to_owned(); MAX_FILTERS], &["1".into()]).is_err()
        );
        assert!(
            scope(&[], &["64496"])?
                .rejects_route("192.0.2.1", 64496)
                .is_err()
        );
        Ok(())
    }
}
