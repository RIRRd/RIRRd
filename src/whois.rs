use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    net::IpAddr,
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    task::JoinSet,
    time::timeout,
};

use crate::{
    config::Config,
    storage::{PrefixMatch, Store},
};

const MAX_QUERY_BYTES: u64 = 512;
const MAX_RESULTS: usize = 16;
const MAX_SESSION_QUERIES: usize = 1024;
const MAX_NRTM_OPERATIONS: usize = 1024;
const MAX_NRTM_TEXT_BYTES: usize = 4 * 1024 * 1024;

struct ConnectionPermit {
    _global: OwnedSemaphorePermit,
    _peer: Option<OwnedSemaphorePermit>,
}

struct Access {
    sources: Arc<[String]>,
    nrtm3: HashMap<String, HashSet<IpAddr>>,
}

fn admit(
    slots: &Arc<Semaphore>,
    peers: &mut HashMap<IpAddr, Weak<Semaphore>>,
    ip: IpAddr,
    per_ip: Option<usize>,
    capacity: usize,
) -> Option<ConnectionPermit> {
    let global = slots.clone().try_acquire_owned().ok()?;
    let mut peer = None;
    if let Some(limit) = per_ip {
        let ip = ip.to_canonical();
        let slots = peers.get(&ip).and_then(Weak::upgrade).unwrap_or_else(|| {
            if peers.len() >= capacity {
                peers.retain(|_, slots| slots.strong_count() > 0);
            }
            let slots = Arc::new(Semaphore::new(limit));
            peers.insert(ip, Arc::downgrade(&slots));
            slots
        });
        peer = Some(slots.try_acquire_owned().ok()?);
    }
    Some(ConnectionPermit {
        _global: global,
        _peer: peer,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Query {
    Exact(String),
    Object {
        class: String,
        key: String,
    },
    Origin {
        class: &'static str,
        origin: String,
    },
    Route {
        class: &'static str,
        prefix: String,
        origins_only: bool,
        mode: PrefixMatch,
        irrd: bool,
    },
    Inverse {
        field: String,
        value: String,
        irrd: bool,
    },
    Set {
        name: String,
        recursive: bool,
    },
    SetRoutes {
        name: String,
        family: Option<bool>,
    },
    Nrtm3 {
        source: String,
        start: u64,
        end: Option<u64>,
    },
    Version,
}

pub async fn serve(
    listener: TcpListener,
    config: &Config,
    store: Arc<Store>,
    mut stopped: watch::Receiver<bool>,
) -> Result<()> {
    let slots = Arc::new(Semaphore::new(config.max_connections));
    let mut peers = HashMap::new();
    let deadline = Duration::from_secs(config.io_timeout_seconds);
    let mut clients = JoinSet::new();
    let access = Arc::new(Access {
        sources: config.sources.clone().into(),
        nrtm3: config.nrtm3_serve.clone(),
    });

    tracing::info!(address = %listener.local_addr()?, "WHOIS listening");
    loop {
        tokio::select! {
            biased;
            _ = stopped.changed() => break,
            Some(result) = clients.join_next(), if !clients.is_empty() => {
                if let Err(error) = result {
                    tracing::error!(%error, "WHOIS connection task failed");
                }
            }
            accepted = listener.accept() => {
                let (socket, address) = accepted?;
                let ip = address.ip().to_canonical();
                let peer_limit = config.max_connections_per_ip
                    .filter(|_| !config.connection_limit_exempt_ips.contains(&ip));
                if let Some(permit) = admit(
                    &slots, &mut peers, ip, peer_limit, config.max_connections,
                ) {
                    clients.spawn(handle(socket, store.clone(), access.clone(), ip, deadline, permit, stopped.clone()));
                }
            }
        }
    }
    drop(listener);
    while clients.join_next().await.is_some() {}
    Ok(())
}

async fn handle(
    socket: TcpStream,
    store: Arc<Store>,
    access: Arc<Access>,
    peer: IpAddr,
    deadline: Duration,
    mut permit: ConnectionPermit,
    mut stopped: watch::Receiver<bool>,
) {
    let (read, mut write) = socket.into_split();
    let mut input = BufReader::new(read);
    let mut selected = access.sources.clone();
    let mut excluded: Arc<[String]> = Arc::new([]);
    let mut persistent = false;
    let mut line = Vec::new();
    for _ in 0..MAX_SESSION_QUERIES {
        line.clear();
        let mut bounded = (&mut input).take(MAX_QUERY_BYTES + 1);
        let read = tokio::select! {
            biased;
            _ = stopped.changed() => return,
            read = timeout(deadline, bounded.read_until(b'\n', &mut line)) => read,
        };
        if !matches!(read, Ok(Ok(n)) if n > 0) {
            return;
        }
        let response = match session_command(&line) {
            Some(SessionCommand::Quit) => return,
            Some(SessionCommand::Persistent) => {
                persistent = true;
                continue;
            }
            Some(SessionCommand::RipePersistent) => {
                persistent = true;
                "%  No entries found for the selected source(s).\n\n\n".to_owned()
            }
            Some(SessionCommand::ListSources) => frame_irrd(selected.join(",")),
            Some(SessionCommand::ListExclusions) => frame_irrd(excluded.join(",")),
            Some(SessionCommand::Exclude(requested)) => match parse_exclusions(requested) {
                Some(names) => {
                    excluded = names.into();
                    "C\n".to_owned()
                }
                None => "F Invalid set exclusion\n".to_owned(),
            },
            Some(SessionCommand::Identify | SessionCommand::Noop) => "C\n".to_owned(),
            Some(SessionCommand::SelectSources(requested)) => {
                match select_sources(requested, &access.sources) {
                    Some(requested) => {
                        selected = requested.into();
                        "C\n".to_owned()
                    }
                    None => "F Invalid source selection\n".to_owned(),
                }
            }
            None => {
                if let Some(query) = parse_query(&line) {
                    if matches!(&query, Query::Nrtm3 { source, .. }
                        if !access.nrtm3.get(source).is_some_and(|peers| peers.contains(&peer)))
                    {
                        "%% ERROR: Access denied\n\n\n".to_owned()
                    } else {
                        let Some((response, returned_permit)) = lookup(
                            query,
                            store.clone(),
                            selected.clone(),
                            excluded.clone(),
                            deadline,
                            permit,
                        )
                        .await
                        else {
                            return;
                        };
                        permit = returned_permit;
                        response
                    }
                } else {
                    if line.trim_ascii_start().starts_with(b"!") {
                        "F Invalid or unsupported query\n".to_owned()
                    } else {
                        "% Invalid or unsupported query\r\n".to_owned()
                    }
                }
            }
        };
        if !matches!(
            timeout(deadline, write.write_all(response.as_bytes())).await,
            Ok(Ok(()))
        ) || !persistent
            || line.len() as u64 > MAX_QUERY_BYTES
        {
            return;
        }
    }
}

enum SessionCommand<'a> {
    Quit,
    Persistent,
    RipePersistent,
    ListSources,
    SelectSources(&'a str),
    ListExclusions,
    Exclude(&'a str),
    Identify,
    Noop,
}

fn session_command(line: &[u8]) -> Option<SessionCommand<'_>> {
    let command = query_line(line)?;
    match command {
        "!!" => Some(SessionCommand::Persistent),
        "!q" | "!Q" => Some(SessionCommand::Quit),
        "-k" => Some(SessionCommand::RipePersistent),
        "!s-lc" => Some(SessionCommand::ListSources),
        "!s-*" => Some(SessionCommand::Noop),
        "!e-lc" => Some(SessionCommand::ListExclusions),
        _ if command.starts_with("!e") => command.strip_prefix("!e").map(SessionCommand::Exclude),
        _ if command.starts_with("!n") && command.len() > 2 => Some(SessionCommand::Identify),
        _ => command
            .strip_prefix("!s")
            .map(SessionCommand::SelectSources),
    }
}

fn select_sources(requested: &str, allowed: &[String]) -> Option<Vec<String>> {
    let mut selected = Vec::new();
    for source in requested.split(',') {
        let source = allowed
            .iter()
            .find(|name| name.eq_ignore_ascii_case(source))?;
        if !selected.contains(source) {
            selected.push(source.clone());
        }
    }
    Some(selected)
}

fn parse_exclusions(value: &str) -> Option<Vec<String>> {
    if value.is_empty() {
        return Some(Vec::new());
    }
    value.split(',').map(parse_set_name).collect()
}

fn parse_set_name(value: &str) -> Option<String> {
    (!value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_:".contains(&byte)))
    .then(|| value.to_ascii_uppercase())
}

async fn lookup(
    query: Query,
    store: Arc<Store>,
    sources: Arc<[String]>,
    excluded: Arc<[String]>,
    deadline: Duration,
    permit: ConnectionPermit,
) -> Option<(String, ConnectionPermit)> {
    let nrtm = matches!(&query, Query::Nrtm3 { .. });
    let irrd = match &query {
        Query::Exact(_) | Query::Nrtm3 { .. } => false,
        Query::Route { irrd, .. } | Query::Inverse { irrd, .. } => *irrd,
        _ => true,
    };
    let lookup = tokio::task::spawn_blocking(move || {
        let result = execute(query, &store, &sources, &excluded);
        (result, permit)
    });
    let response = match timeout(deadline, lookup).await {
        Ok(Ok((Ok(response), permit))) => (response, permit),
        Ok(Ok((Err(error), permit))) => {
            tracing::error!(%error, "WHOIS lookup failed");
            let response = if nrtm {
                "%% ERROR: Requested NRTMv3 range is unavailable\n\n\n"
            } else if irrd {
                "F Lookup failed\n"
            } else {
                "% Lookup failed\r\n"
            };
            (response.to_owned(), permit)
        }
        _ => return None,
    };
    Some(response)
}

fn execute(query: Query, store: &Store, sources: &[String], excluded: &[String]) -> Result<String> {
    match query {
        Query::Version => Ok(frame_irrd(format!(
            "rust-irrd {}",
            env!("CARGO_PKG_VERSION")
        ))),
        Query::Object { class, key } => {
            let Some(object) = store.first(&class, &key, sources)? else {
                return Ok("D\n".to_owned());
            };
            let body = object.public_text().lines().collect::<Vec<_>>().join("\n");
            Ok(frame_irrd(body))
        }
        Query::Origin { class, origin } => {
            let prefixes = store.origin_prefixes(class, &origin, sources, 4097)?;
            if prefixes.is_empty() {
                return Ok("D\n".to_owned());
            }
            if prefixes.len() > 4096 {
                return Ok("F Too many results\n".to_owned());
            }
            Ok(frame_irrd(prefixes.join(" ")))
        }
        Query::Route {
            class,
            prefix,
            origins_only,
            mode,
            irrd,
        } => {
            let routes =
                store.routes_matching_prefix(class, &prefix, mode, sources, MAX_RESULTS + 1)?;
            if !origins_only || routes.is_empty() || routes.len() > MAX_RESULTS {
                return Ok(frame_objects(&routes, irrd));
            }
            let origins = routes
                .iter()
                .map(|route| {
                    route
                        .key
                        .strip_prefix(&prefix)
                        .context("invalid route key in storage")
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(frame_irrd(origins.join(" ")))
        }
        Query::Inverse { field, value, irrd } => {
            let matches = store.inverse(&field, &value, sources, MAX_RESULTS + 1)?;
            Ok(frame_objects(&matches, irrd))
        }
        Query::Set { name, recursive } => {
            let members = crate::sets::members(store, sources, &name, recursive, excluded)?;
            Ok(frame_values(members))
        }
        Query::SetRoutes { name, family } => {
            let prefixes = crate::sets::routes_for_as_set(store, sources, &name, family, excluded)?;
            Ok(frame_values(prefixes))
        }
        Query::Nrtm3 { source, start, end } => {
            let Some((end, operations)) = store.nrtm3_journal(
                &source,
                start,
                end,
                MAX_NRTM_OPERATIONS,
                MAX_NRTM_TEXT_BYTES,
            )?
            else {
                return Ok("% Warning: there are no newer updates available\n\n\n".to_owned());
            };
            let mut reply = format!("%START Version: 3 {source} {start}-{end}\n\n");
            for operation in operations {
                let command = match operation.kind {
                    crate::nrtm3::Kind::Add => "ADD",
                    crate::nrtm3::Kind::Delete => "DEL",
                };
                writeln!(reply, "{command} {}\n", operation.serial)?;
                for line in operation.object.public_text().lines() {
                    writeln!(reply, "{line}")?;
                }
                reply.push('\n');
            }
            writeln!(reply, "%END {source}\n\n")?;
            Ok(reply)
        }
        Query::Exact(key) => {
            let objects = store.exact(&key, sources, MAX_RESULTS + 1)?;
            if objects.len() > MAX_RESULTS {
                return Ok("% Too many results\r\n".to_owned());
            }
            if objects.is_empty() {
                return Ok("% No entries found\r\n".to_owned());
            }
            let mut response = String::new();
            for object in objects {
                for line in object.public_text().lines() {
                    response.push_str(line);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
            }
            Ok(response)
        }
    }
}

fn frame_irrd(body: String) -> String {
    if body.is_empty() {
        return "C\n".to_owned();
    }
    format!("A{}\n{body}\nC\n", body.len() + 1)
}

fn frame_values(values: Vec<String>) -> String {
    if values.is_empty() {
        "D\n".to_owned()
    } else {
        frame_irrd(values.join(" "))
    }
}

fn frame_objects(objects: &[crate::rpsl::Object], irrd: bool) -> String {
    match (objects.len(), irrd) {
        (0, true) => return "D\n".to_owned(),
        (0, false) => return "%  No entries found for the selected source(s).\n\n\n".to_owned(),
        (count, true) if count > MAX_RESULTS => return "F Too many results\n".to_owned(),
        (count, false) if count > MAX_RESULTS => {
            return "%% ERROR: Too many results\n\n\n".to_owned();
        }
        _ => {}
    }
    let body = objects
        .iter()
        .map(|object| object.public_text().lines().collect::<Vec<_>>().join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    if irrd {
        frame_irrd(body)
    } else {
        format!("{body}\n\n\n")
    }
}

fn parse_query(line: &[u8]) -> Option<Query> {
    let line = query_line(line)?;
    if line == "!v" {
        return Some(Query::Version);
    }
    if let Some(origin) = line.strip_prefix("!g") {
        return Some(Query::Origin {
            class: "route",
            origin: parse_origin(origin)?,
        });
    }
    if let Some(origin) = line.strip_prefix("!6") {
        return Some(Query::Origin {
            class: "route6",
            origin: parse_origin(origin)?,
        });
    }
    if let Some(argument) = line.strip_prefix("!r") {
        let (prefix, option) = argument.split_once(',').unwrap_or((argument, ""));
        let (mode, origins_only) = match option {
            "" if !argument.ends_with(',') => (PrefixMatch::Exact, false),
            "o" => (PrefixMatch::Exact, true),
            "l" => (PrefixMatch::ClosestCovering, false),
            "L" => (PrefixMatch::AllCovering, false),
            "M" => (PrefixMatch::MoreSpecific, false),
            _ => return None,
        };
        return route_query(prefix, mode, origins_only, true);
    }
    if let Some(value) = line.strip_prefix("!o") {
        return inverse_query("mnt-by", value, true);
    }
    if let Some(argument) = line.strip_prefix("!i") {
        let (name, recursive) = argument
            .strip_suffix(",1")
            .map_or((argument, false), |name| (name, true));
        return Some(Query::Set {
            name: parse_set_name(name)?,
            recursive,
        });
    }
    if let Some(argument) = line.strip_prefix("!a") {
        let (name, family) = match argument.as_bytes().first() {
            Some(b'4') => (&argument[1..], Some(false)),
            Some(b'6') => (&argument[1..], Some(true)),
            _ => (argument, None),
        };
        return Some(Query::SetRoutes {
            name: parse_set_name(name)?,
            family,
        });
    }
    if let Some(flags) = line.strip_prefix('-') {
        let (flag, argument) = flags.split_once(' ')?;
        let argument = argument.trim();
        if flag == "g" {
            return parse_journal_query(argument);
        }
        if flag == "i" {
            let (field, value) = argument.split_once(' ')?;
            return inverse_query(field, value, false);
        }
        let mode = match flag {
            "x" => PrefixMatch::Exact,
            "l" => PrefixMatch::ClosestCovering,
            "L" => PrefixMatch::AllCovering,
            "M" => PrefixMatch::MoreSpecific,
            _ => return None,
        };
        return route_query(argument, mode, false, false);
    }
    if let Some(argument) = line.strip_prefix("!m") {
        let (class, key) = argument.split_once(',')?;
        let key = key.trim();
        if class.is_empty()
            || !class
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || key.is_empty()
        {
            return None;
        }
        let key = if matches!(class, "route" | "route6") {
            key.replace([' ', '-'], "").to_ascii_uppercase()
        } else {
            key.to_uppercase()
        };
        return Some(Query::Object {
            class: class.to_owned(),
            key,
        });
    }
    (!line.is_empty() && !line.starts_with(['!', '-'])).then(|| Query::Exact(line.to_uppercase()))
}

fn parse_journal_query(argument: &str) -> Option<Query> {
    let (source, range) = argument.split_once(":3:")?;
    if source.len() > 32 || !crate::rpsl::generic_name(source) {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    let serial = |value: &str| {
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| value.parse::<u64>().ok())
            .flatten()
            .filter(|number| *number > 0)
    };
    let start = serial(start)?;
    let end = if end == "LAST" {
        None
    } else {
        Some(serial(end)?)
    };
    if end.is_some_and(|end| end < start) {
        return None;
    }
    Some(Query::Nrtm3 {
        source: source.to_ascii_uppercase(),
        start,
        end,
    })
}

fn route_query(prefix: &str, mode: PrefixMatch, origins_only: bool, irrd: bool) -> Option<Query> {
    let ipv6 = prefix.contains(':');
    let prefix = if prefix.contains('/') {
        prefix.to_owned()
    } else if ipv6 {
        format!("{prefix}/128")
    } else {
        format!("{prefix}/32")
    };
    Some(Query::Route {
        class: if ipv6 { "route6" } else { "route" },
        prefix: crate::rpsl::canonical_prefix(&prefix, ipv6)
            .ok()?
            .to_ascii_uppercase(),
        origins_only,
        mode,
        irrd,
    })
}

fn inverse_query(field: &str, value: &str, irrd: bool) -> Option<Query> {
    let value = value.trim();
    if value.is_empty() || !(field == "origin" || crate::rpsl::is_inverse_field(field)) {
        return None;
    }
    let value = if field == "origin" {
        parse_origin(value)?
    } else {
        value.to_uppercase()
    };
    Some(Query::Inverse {
        field: field.to_owned(),
        value,
        irrd,
    })
}

fn query_line(line: &[u8]) -> Option<&str> {
    if line.len() as u64 > MAX_QUERY_BYTES {
        return None;
    }
    let line = std::str::from_utf8(line.strip_suffix(b"\n")?).ok()?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.chars().any(char::is_control) {
        return None;
    }
    Some(line.trim())
}

fn parse_origin(value: &str) -> Option<String> {
    let value = value.to_ascii_uppercase();
    let digits = value.strip_prefix("AS")?;
    if !digits
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let number = if let Some((high, low)) = digits.split_once('.') {
        let high: u16 = high.parse().ok()?;
        let low: u16 = low.parse().ok()?;
        (u32::from(high) << 16) | u32::from(low)
    } else {
        digits.parse::<u32>().ok()?
    };
    Some(format!("AS{number}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_limit_counts_mapped_addresses_and_releases_after_lookup() {
        let slots = Arc::new(Semaphore::new(2));
        let mut peers = HashMap::new();
        let localhost = "127.0.0.1".parse().unwrap();
        let mapped = "::ffff:127.0.0.1".parse().unwrap();
        let other = "127.0.0.2".parse().unwrap();
        let first = admit(&slots, &mut peers, localhost, Some(1), 2).unwrap();
        assert!(admit(&slots, &mut peers, mapped, Some(1), 2).is_none());
        let second = admit(&slots, &mut peers, other, Some(1), 2).unwrap();
        assert!(admit(&slots, &mut peers, localhost, None, 2).is_none());
        assert!(admit(&slots, &mut peers, "127.0.0.3".parse().unwrap(), Some(1), 2).is_none());
        drop((first, second));
        assert!(admit(&slots, &mut peers, mapped, Some(1), 2).is_some());
        assert!(admit(&slots, &mut peers, "127.0.0.3".parse().unwrap(), Some(1), 2).is_some());
        assert!(peers.len() <= 2);
    }

    #[test]
    fn parses_one_bounded_line_and_normalises_legacy_route_keys() {
        assert_eq!(
            parse_query(b"as64496\r\n"),
            Some(Query::Exact("AS64496".into()))
        );
        assert_eq!(
            parse_query(b"!mroute6,::/0-as64496\n"),
            Some(Query::Object {
                class: "route6".into(),
                key: "::/0AS64496".into(),
            })
        );
        assert_eq!(parse_query(b"!v\n"), Some(Query::Version));
        assert_eq!(
            parse_query("!mperson,nóć@example.net\r\n".as_bytes()),
            Some(Query::Object {
                class: "person".into(),
                key: "NÓĆ@EXAMPLE.NET".into()
            })
        );
        assert_eq!(
            parse_query(b"!6as1.0\n"),
            Some(Query::Origin {
                class: "route6",
                origin: "AS65536".into(),
            })
        );
        assert_eq!(frame_irrd("é".into()), "A3\né\nC\n");
        for invalid in [
            b"-i origin\r\n".as_slice(),
            b"as1",
            b"\r\n",
            b"as1\0\r\n",
            b"!m,AS1\n",
        ] {
            assert_eq!(parse_query(invalid), None);
        }
        assert_eq!(parse_query(&[b'A'; 513]), None);
    }
}
