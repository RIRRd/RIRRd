use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    extract::{Query as HttpQuery, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
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
const MAX_ENCODED_HTTP_QUERY_BYTES: usize = MAX_QUERY_BYTES as usize * 3 + 32;
const MAX_HTTP_LOOKUPS: usize = 8;
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

#[derive(Clone)]
struct Client {
    access: Arc<Access>,
    peer: IpAddr,
}

impl Client {
    fn permits_nrtm3(&self, source: &str) -> bool {
        self.access
            .nrtm3
            .get(source)
            .is_some_and(|peers| peers.contains(&self.peer))
    }
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
    Nrtm3Sources,
    Version,
}

impl Query {
    fn uses_irrd_framing(&self) -> bool {
        match self {
            Self::Exact(_) | Self::Nrtm3 { .. } | Self::Nrtm3Sources => false,
            Self::Route { irrd, .. } | Self::Inverse { irrd, .. } => *irrd,
            _ => true,
        }
    }
}

#[derive(Clone)]
struct HttpWhois {
    client: Client,
    store: Arc<Store>,
    slots: Arc<Semaphore>,
    deadline: Duration,
}

#[derive(Deserialize)]
struct HttpParameters {
    q: Option<String>,
}

/// Serve unframed WHOIS results on the optional HTTP listener. NRTM transfer
/// privileges belong only to the separate TCP listener and its peer ACL.
pub fn http_router(store: Arc<Store>, sources: Vec<String>, deadline: Duration) -> Router {
    let state = HttpWhois {
        client: Client {
            access: Arc::new(Access {
                sources: sources.into(),
                nrtm3: HashMap::new(),
            }),
            peer: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        },
        store,
        slots: Arc::new(Semaphore::new(MAX_HTTP_LOOKUPS)),
        deadline,
    };
    Router::new()
        .route("/v1/whois/", get(http_query))
        .with_state(state)
}

async fn http_query(State(state): State<HttpWhois>, request: Request) -> Response {
    let invalid_query = (StatusCode::BAD_REQUEST, "Invalid or unsupported query");
    if request
        .uri()
        .query()
        .is_some_and(|query| query.len() > MAX_ENCODED_HTTP_QUERY_BYTES)
    {
        return invalid_query.into_response();
    }
    let Ok(HttpQuery(params)) = HttpQuery::<HttpParameters>::try_from_uri(request.uri()) else {
        return invalid_query.into_response();
    };
    let Some(query) = params.q else {
        return (
            StatusCode::BAD_REQUEST,
            "Missing required query parameter \"q\"",
        )
            .into_response();
    };
    if query.contains('\0') {
        return (
            StatusCode::BAD_REQUEST,
            "Queries may not contain null bytes",
        )
            .into_response();
    }
    if query.len() > MAX_QUERY_BYTES as usize || query.chars().any(char::is_control) {
        return invalid_query.into_response();
    }
    let Ok(permit) = state.slots.try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _view = state.store.query_gate().blocking_read();
        execute_http(query.trim(), &state.store, &state.client)
    });
    match timeout(state.deadline, worker).await {
        Ok(Ok(Ok((StatusCode::NO_CONTENT, _)))) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(Ok((status, body)))) => (status, body).into_response(),
        Ok(Ok(Err(error))) => {
            tracing::error!(%error, "HTTP WHOIS lookup failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "An internal error occurred while processing this query.",
            )
                .into_response()
        }
        Ok(Err(_)) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}

fn execute_http(query: &str, store: &Store, client: &Client) -> Result<(StatusCode, String)> {
    let configured = &client.access.sources;
    let mut sources = configured.clone();
    let mut persistent = false;
    let invalid = || {
        (
            StatusCode::BAD_REQUEST,
            "Invalid or unsupported query".into(),
        )
    };
    let no_content = || (StatusCode::NO_CONTENT, String::new());
    let Some(prepared) = ripe_modifiers(query, configured, &mut sources, &mut persistent) else {
        return Ok(invalid());
    };
    if let Some(command) = session_command(prepared) {
        return Ok(match command {
            SessionCommand::ListSources => (StatusCode::OK, sources.join(",")),
            SessionCommand::Persistent
            | SessionCommand::ListExclusions
            | SessionCommand::Identify
            | SessionCommand::Noop => no_content(),
            SessionCommand::SelectSources(names) if select_sources(names, configured).is_some() => {
                no_content()
            }
            SessionCommand::Exclude(names) if parse_exclusions(names).is_some() => no_content(),
            SessionCommand::Timeout(value) if parse_timeout(value).is_some() => no_content(),
            _ => invalid(),
        });
    }
    if prepared.is_empty() && query.starts_with('-') {
        return Ok(no_content());
    }
    let Some(query) = parse_query_text(prepared) else {
        return Ok(invalid());
    };
    if matches!(&query, Query::Nrtm3 { .. }) {
        return Ok((StatusCode::BAD_REQUEST, "Access denied".into()));
    }
    let irrd = query.uses_irrd_framing();
    let response = execute(query, store, &sources, &[], client)?;
    unframe_http(response, irrd)
}

fn unframe_http(response: String, irrd: bool) -> Result<(StatusCode, String)> {
    // The shared resolver generates these exact transport envelopes. Retain
    // its payload while applying the HTTP endpoint's status and body rules.
    if irrd {
        if let Some(error) = response.strip_prefix("F ") {
            return Ok((
                StatusCode::BAD_REQUEST,
                error.trim_end_matches('\n').to_owned(),
            ));
        }
        return match response.as_str() {
            "C\n" | "D\n" => Ok((StatusCode::NO_CONTENT, String::new())),
            response => {
                let (length, framed_body) = response
                    .strip_prefix('A')
                    .and_then(|response| response.split_once('\n'))
                    .context("invalid internal WHOIS success header")?;
                let body = framed_body
                    .strip_suffix("\nC\n")
                    .context("invalid internal WHOIS success trailer")?;
                ensure!(
                    length.parse::<usize>()? == body.len() + 1,
                    "invalid internal WHOIS byte count"
                );
                Ok((StatusCode::OK, body.to_owned()))
            }
        };
    }
    if let Some(error) = response.strip_prefix("%% ERROR: ") {
        return Ok((
            StatusCode::BAD_REQUEST,
            error.trim_end_matches('\n').to_owned(),
        ));
    }
    match response.as_str() {
        "% No entries found\r\n" | "%  No entries found for the selected source(s).\n\n\n" => {
            Ok((StatusCode::NO_CONTENT, String::new()))
        }
        "% Too many results\r\n" => Ok((StatusCode::BAD_REQUEST, "Too many results".into())),
        response => {
            let body = if let Some(body) = response.strip_suffix("\n\n\n") {
                body.to_owned()
            } else {
                response
                    .strip_suffix("\r\n\r\n")
                    .context("invalid internal WHOIS response terminator")?
                    .replace("\r\n", "\n")
            };
            Ok((StatusCode::OK, body))
        }
    }
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
                    let client = Client { access: access.clone(), peer: ip };
                    clients.spawn(handle(socket, store.clone(), client, deadline, permit, stopped.clone()));
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
    client: Client,
    deadline: Duration,
    mut permit: ConnectionPermit,
    mut stopped: watch::Receiver<bool>,
) {
    let access = &client.access;
    let (read, mut write) = socket.into_split();
    let mut input = BufReader::new(read);
    let mut selected = access.sources.clone();
    let mut excluded: Arc<[String]> = Arc::new([]);
    let mut persistent = false;
    let mut idle = deadline;
    let mut line = Vec::new();
    for _ in 0..MAX_SESSION_QUERIES {
        line.clear();
        let mut bounded = (&mut input).take(MAX_QUERY_BYTES + 1);
        let read = tokio::select! {
            biased;
            _ = stopped.changed() => return,
            read = timeout(idle, bounded.read_until(b'\n', &mut line)) => read,
        };
        if !matches!(read, Ok(Ok(n)) if n > 0) {
            return;
        }
        let text = query_line(&line);
        let prepared = text.and_then(|query| {
            ripe_modifiers(query, &access.sources, &mut selected, &mut persistent)
        });
        let response = match prepared.and_then(session_command) {
            Some(SessionCommand::Quit) => return,
            Some(SessionCommand::Persistent) => {
                persistent = true;
                continue;
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
            Some(SessionCommand::Timeout(value)) => match parse_timeout(value) {
                Some(requested) => {
                    idle = requested.min(deadline);
                    "C\n".to_owned()
                }
                None if value.is_empty() => "F Missing parameter for t query\n".to_owned(),
                None => format!("F Invalid value for timeout: {value}\n"),
            },
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
                if prepared == Some("") && text.is_some_and(|query| query.starts_with('-')) {
                    "%  No entries found for the selected source(s).\n\n\n".to_owned()
                } else if let Some(query) = prepared.and_then(parse_query_text) {
                    if matches!(&query, Query::Nrtm3 { source, .. }
                        if !client.permits_nrtm3(source))
                    {
                        "%% ERROR: Access denied\n\n\n".to_owned()
                    } else {
                        let Some((response, returned_permit)) = lookup(
                            query,
                            store.clone(),
                            selected.clone(),
                            excluded.clone(),
                            client.clone(),
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
                    if prepared.is_some_and(|query| query.starts_with('!'))
                        || line.trim_ascii_start().starts_with(b"!")
                    {
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
    ListSources,
    SelectSources(&'a str),
    ListExclusions,
    Exclude(&'a str),
    Timeout(&'a str),
    Identify,
    Noop,
}

fn session_command(command: &str) -> Option<SessionCommand<'_>> {
    match command {
        "!!" => Some(SessionCommand::Persistent),
        "!q" | "!Q" => Some(SessionCommand::Quit),
        "!s-lc" => Some(SessionCommand::ListSources),
        "!s-*" => Some(SessionCommand::Noop),
        "!e-lc" => Some(SessionCommand::ListExclusions),
        _ if command.starts_with("!e") => command.strip_prefix("!e").map(SessionCommand::Exclude),
        _ if command.starts_with("!t") => command.strip_prefix("!t").map(SessionCommand::Timeout),
        _ if command.starts_with("!n") && command.len() > 2 => Some(SessionCommand::Identify),
        _ => command
            .strip_prefix("!s")
            .map(SessionCommand::SelectSources),
    }
}

fn parse_timeout(value: &str) -> Option<Duration> {
    let seconds = value.trim().parse::<u64>().ok()?;
    (1..=1000)
        .contains(&seconds)
        .then(|| Duration::from_secs(seconds))
}

fn ripe_modifiers<'a>(
    mut query: &'a str,
    allowed: &Arc<[String]>,
    selected: &mut Arc<[String]>,
    persistent: &mut bool,
) -> Option<&'a str> {
    if !query.starts_with('-') {
        return Some(query);
    }
    let mut next_sources = None;
    let mut keep_alive = false;
    let mut ripe_only = false;
    loop {
        let (flag, argument) = word(query);
        match flag {
            "-s" => {
                let (names, rest) = word(argument);
                next_sources = Some(select_sources(names, allowed)?.into());
                query = rest;
            }
            "-a" => {
                next_sources = Some(allowed.clone());
                query = argument;
            }
            "-k" => {
                keep_alive = true;
                query = argument;
            }
            "-F" | "-r" => query = argument,
            "-V" => {
                let (agent, rest) = word(argument);
                if agent.is_empty() {
                    return None;
                }
                query = rest;
                continue;
            }
            _ => break,
        }
        ripe_only = true;
    }
    if ripe_only && query.starts_with('!') {
        return None;
    }
    if let Some(next_sources) = next_sources {
        *selected = next_sources;
    }
    *persistent |= keep_alive;
    Some(query)
}

fn word(line: &str) -> (&str, &str) {
    line.split_once(' ')
        .map_or((line, ""), |(head, rest)| (head, rest.trim_start()))
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
    client: Client,
    deadline: Duration,
    permit: ConnectionPermit,
) -> Option<(String, ConnectionPermit)> {
    let nrtm = matches!(&query, Query::Nrtm3 { .. });
    let discovery = matches!(&query, Query::Nrtm3Sources);
    let irrd = query.uses_irrd_framing();
    let lookup = tokio::task::spawn_blocking(move || {
        let _view = store.query_gate().blocking_read();
        let result = execute(query, &store, &sources, &excluded, &client);
        (result, permit)
    });
    let response = match timeout(deadline, lookup).await {
        Ok(Ok((Ok(response), permit))) => (response, permit),
        Ok(Ok((Err(error), permit))) => {
            tracing::error!(%error, "WHOIS lookup failed");
            let response = if nrtm {
                "%% ERROR: Requested NRTMv3 range is unavailable\n\n\n"
            } else if discovery {
                "%% ERROR: An internal error occurred while processing this query.\n\n\n"
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

fn execute(
    query: Query,
    store: &Store,
    sources: &[String],
    excluded: &[String],
    client: &Client,
) -> Result<String> {
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
        Query::Nrtm3Sources => nrtm3_sources(store, client),
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

fn nrtm3_sources(store: &Store, client: &Client) -> Result<String> {
    let mut response = String::new();
    for source in client.access.sources.iter() {
        let permission = if client.permits_nrtm3(source) {
            "X"
        } else {
            "N"
        };
        if let Some((oldest, newest)) = store.nrtm3_journal_bounds(source)? {
            writeln!(response, "{source}:3:{permission}:{oldest}-{newest}")?;
        } else {
            writeln!(response, "{source}:3:{permission}:Unknown")?;
        }
    }
    response.push_str("\n\n");
    Ok(response)
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

#[cfg(test)]
fn parse_query(line: &[u8]) -> Option<Query> {
    parse_query_text(query_line(line)?)
}

fn parse_query_text(line: &str) -> Option<Query> {
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
        if flag == "q" && argument == "sources" {
            return Some(Query::Nrtm3Sources);
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

    #[tokio::test]
    async fn tcp_timeout_command_changes_only_the_sessions_idle_read_ceiling() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let config_file = directory.path().join("irrd.toml");
        std::fs::write(
            &config_file,
            r#"
database = "registry.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 3
max_snapshot_bytes = 4096
"#,
        )?;
        let config = Config::load(&config_file)?;
        let store = Arc::new(Store::open(&config.database)?);
        let listener = TcpListener::bind(config.listen).await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = watch::channel(false);
        let server = tokio::spawn(async move { serve(listener, &config, store, stopped).await });

        let mut short = BufReader::new(TcpStream::connect(address).await?);
        short
            .get_mut()
            .write_all(b"!!\r\n!t1\r\n!t0\r\n!t1001\r\n!t\r\n")
            .await?;
        let mut line = String::new();
        for expected in [
            "C\n",
            "F Invalid value for timeout: 0\n",
            "F Invalid value for timeout: 1001\n",
            "F Missing parameter for t query\n",
        ] {
            line.clear();
            timeout(Duration::from_secs(2), short.read_line(&mut line)).await??;
            assert_eq!(line, expected);
        }

        let mut capped = BufReader::new(TcpStream::connect(address).await?);
        capped.get_mut().write_all(b"!!\n!t1000\n").await?;
        line.clear();
        timeout(Duration::from_secs(2), capped.read_line(&mut line)).await??;
        assert_eq!(line, "C\n");

        let mut standalone = TcpStream::connect(address).await?;
        standalone.write_all(b"!t1\n").await?;
        line.clear();
        timeout(Duration::from_secs(2), standalone.read_to_string(&mut line)).await??;
        assert_eq!(line, "C\n");

        line.clear();
        assert_eq!(
            timeout(Duration::from_millis(2500), short.read_line(&mut line)).await??,
            0
        );
        capped.get_mut().write_all(b"!s-lc\n").await?;
        let mut frame = [0; 10];
        timeout(Duration::from_secs(2), capped.read_exact(&mut frame)).await??;
        assert_eq!(&frame, b"A5\nTEST\nC\n");
        assert_eq!(
            timeout(Duration::from_secs(5), capped.read_line(&mut line)).await??,
            0
        );

        let _ = stop.send(true);
        timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }

    #[tokio::test]
    async fn advertises_nrtm3_ranges_with_per_peer_acl_and_ripe_framing() -> Result<()> {
        use crate::nrtm3::{Kind, Operation};

        let directory = tempfile::tempdir()?;
        let config_file = directory.path().join("irrd.toml");
        std::fs::write(
            &config_file,
            r#"
database = "registry.redb"
sources = ["OTHER", "EMPTY", "TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096

[nrtm3_serve]
OTHER = ["192.0.2.1"]
TEST = ["127.0.0.1"]
"#,
        )?;
        let config = Config::load(&config_file)?;
        let store = Arc::new(Store::open(&config.database)?);
        let writer = Arc::clone(&store);
        tokio::task::spawn_blocking(move || -> Result<()> {
            for (source, first, last) in [("OTHER", 31, 31), ("TEST", 41, 42)] {
                let object = crate::rpsl::parse(&format!("aut-num: AS64496\nsource: {source}"))
                    .map_err(anyhow::Error::msg)?;
                writer.replace_source(source, [Ok(object.clone())])?;
                writer.apply_nrtm3(
                    source,
                    (first, last),
                    Some(first - 1),
                    (first..=last).map(|serial| {
                        Ok(Operation {
                            kind: Kind::Add,
                            serial,
                            object: object.clone(),
                        })
                    }),
                )?;
            }
            Ok(())
        })
        .await??;

        let listener = TcpListener::bind(config.listen).await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = watch::channel(false);
        let server = tokio::spawn(async move { serve(listener, &config, store, stopped).await });
        for (query, expected) in [
            (
                "-q sources\r\n",
                "OTHER:3:N:31-31\nEMPTY:3:N:Unknown\nTEST:3:X:41-42\n\n\n",
            ),
            (
                "-s TEST -q sources\r\n",
                "OTHER:3:N:31-31\nEMPTY:3:N:Unknown\nTEST:3:X:41-42\n\n\n",
            ),
            ("-g OTHER:3:31-LAST\r\n", "%% ERROR: Access denied\n\n\n"),
            (
                "-g TEST:3:43-LAST\r\n",
                "% Warning: there are no newer updates available\n\n\n",
            ),
        ] {
            let mut socket = TcpStream::connect(address).await?;
            socket.write_all(query.as_bytes()).await?;
            let mut response = String::new();
            timeout(Duration::from_secs(3), socket.read_to_string(&mut response)).await??;
            assert_eq!(response, expected);
        }
        let _ = stop.send(true);
        timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }

    #[tokio::test]
    async fn http_whois_returns_unframed_results_without_mirror_privileges() -> Result<()> {
        use crate::nrtm3::{Kind, Operation};

        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("http-whois.redb"))?);
        let writer = Arc::clone(&store);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let record = |text| crate::rpsl::parse(text).map_err(anyhow::Error::msg);
            writer.replace_source(
                "TEST",
                [
                    record("aut-num: AS64496\r\nsource: TEST\r\n"),
                    record("mntner: TEST-MNT\nauth: BCRYPT-PW hidden-value\n+ hidden-fold\nsource: TEST"),
                ],
            )?;
            writer.apply_nrtm3(
                "TEST",
                (11, 11),
                Some(10),
                [Ok(Operation {
                    kind: Kind::Add,
                    serial: 11,
                    object: record("aut-num: AS64497\nsource: TEST")?,
                })],
            )?;
            writer.replace_source(
                "OTHER",
                [record("route: 192.0.2.0/24\norigin: AS64496\nsource: OTHER")],
            )?;
            Ok(())
        })
        .await??;

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                http_router(
                    store,
                    vec!["TEST".into(), "OTHER".into()],
                    Duration::from_secs(2),
                ),
            )
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
        });

        for (query, status, expected) in [
            ("", 400, "Missing required query parameter \"q\""),
            (
                "?q=%21maut-num%2Cas64496",
                200,
                "aut-num: AS64496\nsource: TEST",
            ),
            ("?q=AS64496", 200, "aut-num: AS64496\nsource: TEST"),
            (
                "?q=%21mmntner%2CTEST-MNT",
                200,
                "mntner: TEST-MNT\nsource: TEST",
            ),
            (
                "?q=-s+OTHER+-x+192.0.2.0%2F24",
                200,
                "route: 192.0.2.0/24\norigin: AS64496\nsource: OTHER",
            ),
            ("?q=%21maut-num%2CAS64500", 204, ""),
            ("?q=AS64500", 204, ""),
            ("?q=%21t10", 204, ""),
            ("?q=%21t1001", 400, "Invalid or unsupported query"),
            ("?q=%21%21", 204, ""),
            ("?q=%21s-lc", 200, "TEST,OTHER"),
            ("?q=%21v%00", 400, "Queries may not contain null bytes"),
            ("?q=%21v%0A%21v", 400, "Invalid or unsupported query"),
            ("?q=-g+TEST%3A3%3A11-LAST", 400, "Access denied"),
            ("?q=-q+sources", 200, "TEST:3:N:11-11\nOTHER:3:N:Unknown"),
        ] {
            let mut socket = TcpStream::connect(address).await?;
            let request = format!(
                "GET /v1/whois/{query} HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 127.0.0.1\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(request.as_bytes()).await?;
            let mut response = String::new();
            timeout(Duration::from_secs(3), socket.read_to_string(&mut response)).await??;
            let (headers, body) = response
                .split_once("\r\n\r\n")
                .context("malformed HTTP response")?;
            assert!(
                headers.starts_with(&format!("HTTP/1.1 {status} ")),
                "{headers}"
            );
            assert_eq!(body, expected, "{query}");
        }
        let _ = stop.send(());
        timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }
}
