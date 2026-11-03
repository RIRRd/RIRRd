//! Bounded read-only GraphQL queries over the shared store.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    future::Future,
    net::IpAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use async_graphql::{
    Error, Request, Value,
    dynamic::{
        Field, FieldFuture, FieldValue, InputValue, Interface, InterfaceField,
        Object as GraphqlObject, ResolverContext, Scalar, Schema, TypeRef,
    },
};
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Request as HttpRequest, State},
    http::{StatusCode, header::HOST, uri::Authority},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use tokio::{net::TcpListener, sync::Semaphore, task::spawn_blocking};
use zeroize::Zeroize;

use crate::{
    rpsl::Object,
    sets::{self, recursive_members_at_root, routes_for_as_set},
    storage::Store,
};

const MAX_HTTP_BYTES: usize = 16 * 1024;
const MAX_HTTP_REQUESTS: usize = 8;
const MAX_LOOKUPS: usize = 8;
const MAX_RECORDS: usize = 8;
const MAX_SET_NAMES: usize = 8;
const MAX_PREFIXES: usize = 4096;
const MAX_SET_ROOTS: usize = 32;
const MAX_SET_MEMBERS: usize = 4096;
const MAX_SET_MEMBER_BYTES: usize = 256 * 1024;
const MAX_FILTER_VALUES: usize = 32;
const MAX_FILTER_BYTES: usize = 512;
const MAX_COMPLEXITY: usize = 24;
const MAX_DEPTH: usize = 8;

const OBJECT_INTERFACE: &str = "RPSLObject";
const OBJECT_TYPES: &[(&str, &str)] = &[
    ("as-block", "RPSLAsBlock"),
    ("as-set", "RPSLAsSet"),
    ("aut-num", "RPSLAutNum"),
    ("domain", "RPSLDomain"),
    ("filter-set", "RPSLFilterSet"),
    ("inet-rtr", "RPSLInetRtr"),
    ("inet6num", "RPSLInet6Num"),
    ("inetnum", "RPSLInetnum"),
    ("irt", "RPSLIrt"),
    ("key-cert", "RPSLKeyCert"),
    ("mntner", "RPSLMntner"),
    ("organisation", "RPSLOrganisation"),
    ("peering-set", "RPSLPeeringSet"),
    ("person", "RPSLPerson"),
    ("role", "RPSLRole"),
    ("route", "RPSLRoute"),
    ("route-set", "RPSLRouteSet"),
    ("route6", "RPSLRoute6"),
    ("rtr-set", "RPSLRtrSet"),
];

#[derive(Clone, Copy)]
enum Output {
    Key,
    Class,
    Source,
    Text,
    RoutePrefix,
    RouteOrigin,
    First(&'static str),
    AsnText(&'static str),
    Repeated(&'static str),
    CommaList(&'static str),
}

const COMMON_FIELDS: &[(&str, Output)] = &[
    ("rpslPk", Output::Key),
    ("objectClass", Output::Class),
    ("source", Output::Source),
    ("objectText", Output::Text),
    ("changed", Output::Repeated("changed")),
    ("mntBy", Output::CommaList("mnt-by")),
    ("notify", Output::Repeated("notify")),
    ("remarks", Output::Repeated("remarks")),
];

fn class_fields(class: &str) -> &'static [(&'static str, Output)] {
    match class {
        "aut-num" => &[
            ("autNum", Output::Key),
            ("asName", Output::First("as-name")),
            ("descr", Output::Repeated("descr")),
            ("memberOf", Output::CommaList("member-of")),
        ],
        "as-set" | "route-set" => &[
            ("members", Output::CommaList("members")),
            ("mbrsByRef", Output::CommaList("mbrs-by-ref")),
            ("descr", Output::Repeated("descr")),
        ],
        "filter-set" => &[
            ("filter", Output::First("filter")),
            ("mpFilter", Output::First("mp-filter")),
            ("descr", Output::Repeated("descr")),
            ("adminC", Output::CommaList("admin-c")),
            ("techC", Output::CommaList("tech-c")),
        ],
        "peering-set" => &[
            ("peering", Output::Repeated("peering")),
            ("mpPeering", Output::Repeated("mp-peering")),
            ("descr", Output::Repeated("descr")),
            ("adminC", Output::CommaList("admin-c")),
            ("techC", Output::CommaList("tech-c")),
        ],
        "rtr-set" => &[
            ("members", Output::CommaList("members")),
            ("mpMembers", Output::CommaList("mp-members")),
            ("mbrsByRef", Output::CommaList("mbrs-by-ref")),
            ("descr", Output::Repeated("descr")),
            ("adminC", Output::CommaList("admin-c")),
            ("techC", Output::CommaList("tech-c")),
        ],
        "inet-rtr" => &[
            ("localAs", Output::AsnText("local-as")),
            ("alias", Output::CommaList("alias")),
            ("ifaddr", Output::Repeated("ifaddr")),
            ("interface", Output::Repeated("interface")),
            ("peer", Output::Repeated("peer")),
            ("mpPeer", Output::Repeated("mp-peer")),
            ("memberOf", Output::CommaList("member-of")),
            ("rsIn", Output::First("rs-in")),
            ("rsOut", Output::First("rs-out")),
            ("descr", Output::Repeated("descr")),
            ("adminC", Output::CommaList("admin-c")),
            ("techC", Output::CommaList("tech-c")),
        ],
        "route" | "route6" => &[
            ("origin", Output::RouteOrigin),
            ("descr", Output::Repeated("descr")),
            ("memberOf", Output::CommaList("member-of")),
        ],
        "organisation" => &[
            ("orgName", Output::First("org-name")),
            ("country", Output::Repeated("country")),
            ("address", Output::Repeated("address")),
            ("phone", Output::Repeated("phone")),
            ("faxNo", Output::Repeated("fax-no")),
            ("eMail", Output::Repeated("e-mail")),
            ("adminC", Output::CommaList("admin-c")),
            ("techC", Output::CommaList("tech-c")),
        ],
        "mntner" => &[("descr", Output::Repeated("descr"))],
        "person" | "role" => &[("nicHdl", Output::Key)],
        _ => &[],
    }
}

fn camel_case(attribute: &str) -> String {
    let mut words = attribute.split('-');
    let mut result = words.next().unwrap_or_default().to_owned();
    for word in words {
        let (head, tail) = word.split_at(1);
        result.push_str(&head.to_ascii_uppercase());
        result.push_str(tail);
    }
    result
}

#[derive(Clone)]
struct Lookup {
    store: Arc<Store>,
    sources: Arc<[String]>,
    authoritative_sources: Arc<HashSet<String>>,
    slots: Arc<Semaphore>,
}

impl Lookup {
    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> async_graphql::Result<T> {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| Error::new("Too many concurrent lookups."))?;
        let store = Arc::clone(&self.store);
        spawn_blocking(move || {
            let _permit = permit;
            work(&store)
        })
        .await
        .context("GraphQL lookup worker failed")
        .and_then(|result| result)
        .map_err(|error| {
            tracing::error!(%error, "GraphQL lookup failed");
            Error::new("Lookup failed.")
        })
    }
}

#[derive(Clone)]
struct HttpLimits {
    slots: Arc<Semaphore>,
    timeout: Duration,
}

struct VisibleObject {
    object: Object,
    text: String,
    attributes: OnceLock<HashMap<String, Vec<String>>>,
}

impl VisibleObject {
    fn new(mut object: Object) -> Self {
        let text = match object.public_text() {
            Cow::Borrowed(_) => std::mem::take(&mut object.text),
            Cow::Owned(public) => {
                // Keep only public text in the resolver parent. The unused raw
                // buffer can contain maintainer credentials.
                let mut raw = std::mem::take(&mut object.text);
                raw.zeroize();
                public
            }
        };
        Self {
            object,
            text,
            attributes: OnceLock::new(),
        }
    }

    fn attributes(&self) -> &HashMap<String, Vec<String>> {
        self.attributes.get_or_init(|| {
            let mut attributes = HashMap::<String, Vec<String>>::new();
            let mut current: Option<(String, String)> = None;
            for line in self.text.lines() {
                if line.starts_with([' ', '\t', '+']) {
                    if let Some((_, value)) = &mut current {
                        value.push(' ');
                        value.push_str(line[1..].split('#').next().unwrap_or_default().trim());
                    }
                    continue;
                }
                if let Some((name, value)) = current.take() {
                    attributes.entry(name).or_default().push(value);
                }
                current = line.split_once(':').map(|(name, value)| {
                    (
                        name.to_ascii_lowercase(),
                        value
                            .split('#')
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .to_owned(),
                    )
                });
            }
            if let Some((name, value)) = current {
                attributes.entry(name).or_default().push(value);
            }
            attributes
        })
    }

    fn values(&self, attribute: &str) -> impl Iterator<Item = &str> {
        self.attributes()
            .get(attribute)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

impl Output {
    fn graphql_type(self) -> TypeRef {
        match self {
            Self::Repeated(_) | Self::CommaList(_) => TypeRef::named_nn_list(TypeRef::STRING),
            _ => TypeRef::named(TypeRef::STRING),
        }
    }

    fn value(self, parent: &VisibleObject) -> Option<FieldValue<'_>> {
        match self {
            Self::Key => Some(FieldValue::value(parent.object.key.clone())),
            Self::Class => Some(FieldValue::value(parent.object.class.clone())),
            Self::Source => Some(FieldValue::value(parent.object.source.clone())),
            Self::Text => Some(FieldValue::value(parent.text.clone())),
            Self::RoutePrefix => parent
                .object
                .key
                .rsplit_once("AS")
                .map(|(prefix, _)| FieldValue::value(prefix.to_ascii_lowercase())),
            Self::RouteOrigin => parent
                .object
                .key
                .rsplit_once("AS")
                .map(|(_, asn)| FieldValue::value(format!("AS{asn}"))),
            Self::First(attribute) => parent.values(attribute).next().map(FieldValue::value),
            Self::AsnText(attribute) => parent.values(attribute).next().and_then(|value| {
                let prefix = value.get(..2)?;
                if !prefix.eq_ignore_ascii_case("AS") {
                    return None;
                }
                let number = value.get(2..)?.parse::<u32>().ok()?;
                Some(FieldValue::value(format!("AS{number}")))
            }),
            Self::Repeated(attribute) => parent.attributes().get(attribute).map(|values| {
                FieldValue::value(Value::List(
                    values.iter().cloned().map(Value::String).collect(),
                ))
            }),
            Self::CommaList(attribute) => parent.attributes().get(attribute).map(|values| {
                FieldValue::value(Value::List(
                    values
                        .iter()
                        .flat_map(|value| value.split(',').map(str::trim))
                        .filter(|value| !value.is_empty())
                        .map(|value| Value::String(value.to_ascii_uppercase()))
                        .collect(),
                ))
            }),
        }
    }

    fn field(self, name: impl Into<String>) -> Field {
        Field::new(name, self.graphql_type(), move |ctx| {
            FieldFuture::new(async move {
                let parent = ctx.parent_value.try_downcast_ref::<VisibleObject>()?;
                Ok(self.value(parent))
            })
        })
    }
}

fn type_name(class: &str) -> Option<&'static str> {
    OBJECT_TYPES
        .iter()
        .find_map(|&(name, type_name)| (class == name).then_some(type_name))
}

fn list_filter(
    ctx: &ResolverContext<'_>,
    name: &str,
) -> async_graphql::Result<Option<Vec<String>>> {
    let Some(value) = ctx.args.get(name).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let list = value.list()?;
    if list.len() > MAX_FILTER_VALUES {
        return Err(Error::new(format!("Too many values for {name}.")));
    }
    let mut values = Vec::with_capacity(list.len());
    for entry in list.iter() {
        let value = entry.string()?.trim();
        if value.is_empty() || value.len() > MAX_FILTER_BYTES || value.chars().any(char::is_control)
        {
            return Err(Error::new(format!("Invalid value for {name}.")));
        }
        values.push(value.to_owned());
    }
    Ok(Some(values))
}

fn source_filter(
    ctx: &ResolverContext<'_>,
    configured: &[String],
) -> async_graphql::Result<Vec<String>> {
    let sources = list_filter(ctx, "sources")?.map_or_else(
        || configured.to_vec(),
        |mut sources| {
            sources
                .iter_mut()
                .for_each(|source| source.make_ascii_uppercase());
            sources
        },
    );
    if sources.iter().any(|source| !configured.contains(source)) {
        return Err(Error::new("Unknown source."));
    }
    Ok(sources)
}

async fn objects(ctx: ResolverContext<'_>) -> async_graphql::Result<Option<FieldValue<'_>>> {
    let lookup = ctx.data::<Lookup>()?;
    let Some(keys) = list_filter(&ctx, "rpslPk")? else {
        return Err(Error::new("Your query must be more specific."));
    };

    let classes = list_filter(&ctx, "objectClass")?;
    let sources = source_filter(&ctx, &lookup.sources)?;

    let limit = ctx
        .args
        .get("recordLimit")
        .filter(|value| !value.is_null())
        .map(|value| value.i64())
        .transpose()?
        .unwrap_or(MAX_RECORDS as i64);
    let limit = usize::try_from(limit)
        .ok()
        .filter(|&limit| limit <= MAX_RECORDS)
        .ok_or_else(|| Error::new(format!("recordLimit must be between 0 and {MAX_RECORDS}.")))?;

    if keys.is_empty()
        || sources.is_empty()
        || classes.as_ref().is_some_and(Vec::is_empty)
        || limit == 0
    {
        return Ok(Some(FieldValue::list(Vec::<FieldValue<'_>>::new())));
    }

    let records = lookup
        .run(move |store| store.exact_objects(&keys, &sources, &classes.unwrap_or_default(), limit))
        .await?;

    let objects = records
        .into_iter()
        .map(|record| {
            let class = type_name(&record.class)
                .ok_or_else(|| Error::new("Unknown stored object class."))?;
            Ok(FieldValue::owned_any(VisibleObject::new(record)).with_type(class))
        })
        .collect::<async_graphql::Result<Vec<_>>>()?;
    Ok(Some(FieldValue::list(objects)))
}

struct Prefixes {
    key: Value,
    prefixes: Vec<String>,
}

fn prefix_type(name: &str, key: &str, key_type: &str) -> GraphqlObject {
    GraphqlObject::new(name)
        .field(Field::new(key, TypeRef::named_nn(key_type), |ctx| {
            FieldFuture::new(async move {
                let row = ctx.parent_value.try_downcast_ref::<Prefixes>()?;
                Ok(Some(FieldValue::value(row.key.clone())))
            })
        }))
        .field(Field::new(
            "prefixes",
            TypeRef::named_nn_list("IP"),
            |ctx| {
                FieldFuture::new(async move {
                    let row = ctx.parent_value.try_downcast_ref::<Prefixes>()?;
                    Ok(Some(FieldValue::list(
                        row.prefixes.iter().map(FieldValue::value),
                    )))
                })
            },
        ))
}

fn asn_value(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => u32::try_from(number.as_u64()?).ok(),
        Value::String(number) if number.bytes().all(|byte| byte.is_ascii_digit()) => {
            number.parse().ok()
        }
        _ => None,
    }
}

fn ip_family(ctx: &ResolverContext<'_>) -> async_graphql::Result<Option<bool>> {
    match ctx
        .args
        .get("ipVersion")
        .filter(|value| !value.is_null())
        .map(|value| value.i64())
        .transpose()?
    {
        None | Some(0) => Ok(None),
        Some(4) => Ok(Some(false)),
        Some(6) => Ok(Some(true)),
        Some(_) => Err(Error::new("ipVersion must be 4 or 6.")),
    }
}

fn prefix_values(
    rows: Option<Vec<Prefixes>>,
) -> async_graphql::Result<Option<FieldValue<'static>>> {
    let rows =
        rows.ok_or_else(|| Error::new(format!("Result exceeds {MAX_PREFIXES} prefixes.")))?;
    Ok(Some(FieldValue::list(
        rows.into_iter().map(FieldValue::owned_any),
    )))
}

async fn asn_prefixes(ctx: ResolverContext<'_>) -> async_graphql::Result<Option<FieldValue<'_>>> {
    let lookup = ctx.data::<Lookup>()?;
    let asns = ctx.args.try_get("asns")?.list()?;
    if asns.len() > MAX_FILTER_VALUES {
        return Err(Error::new("Too many values for asns."));
    }
    let asns = asns
        .iter()
        .map(|asn| asn_value(asn.as_value()).ok_or_else(|| Error::new("Invalid ASN.")))
        .collect::<async_graphql::Result<Vec<_>>>()?;
    let sources = source_filter(&ctx, &lookup.sources)?;
    let family = ip_family(&ctx)?;
    let rows = lookup
        .run(move |store| {
            let mut remaining = MAX_PREFIXES;
            let mut rows = Vec::with_capacity(asns.len());
            for asn in asns {
                let origin = format!("AS{asn}");
                let mut prefixes = Vec::new();
                for (class, ipv6) in [("route", false), ("route6", true)] {
                    if family.is_none_or(|requested| requested == ipv6) {
                        let routes =
                            store.origin_prefixes(class, &origin, &sources, remaining + 1)?;
                        if routes.len() > remaining {
                            return Ok(None);
                        }
                        remaining -= routes.len();
                        prefixes.extend(routes);
                    }
                }
                rows.push(Prefixes {
                    key: Value::from(asn),
                    prefixes,
                });
            }
            Ok(Some(rows))
        })
        .await?;
    prefix_values(rows)
}

fn set_names(ctx: &ResolverContext<'_>) -> async_graphql::Result<Vec<String>> {
    let mut names = list_filter(ctx, "setNames")?.ok_or_else(|| Error::new("Missing setNames."))?;
    if names.len() > MAX_SET_NAMES {
        return Err(Error::new("Too many values for setNames."));
    }
    if ctx
        .args
        .get("sqlTrace")
        .filter(|value| !value.is_null())
        .map(|value| value.boolean())
        .transpose()?
        .unwrap_or(false)
    {
        return Err(Error::new("SQL tracing is unavailable."));
    }
    let mut seen = HashSet::new();
    names.retain_mut(|name| {
        name.make_ascii_uppercase();
        seen.insert(name.clone())
    });
    Ok(names)
}

async fn as_set_prefixes(
    ctx: ResolverContext<'_>,
) -> async_graphql::Result<Option<FieldValue<'_>>> {
    let lookup = ctx.data::<Lookup>()?;
    let names = set_names(&ctx)?;
    let sources = source_filter(&ctx, &lookup.sources)?;
    let family = ip_family(&ctx)?;
    let excluded = list_filter(&ctx, "excludeSets")?.unwrap_or_default();
    let rows = lookup
        .run(move |store| {
            let mut remaining = MAX_PREFIXES;
            let mut rows = Vec::with_capacity(names.len());
            for name in names {
                let prefixes = routes_for_as_set(store, &sources, &name, family, &excluded)?;
                if prefixes.len() > remaining {
                    return Ok(None);
                }
                remaining -= prefixes.len();
                rows.push(Prefixes {
                    key: Value::from(name),
                    prefixes,
                });
            }
            Ok(Some(rows))
        })
        .await?;
    prefix_values(rows)
}

struct SetMembers {
    rpsl_pk: String,
    root_source: String,
    members: Vec<String>,
}

fn set_members_type() -> GraphqlObject {
    GraphqlObject::new("SetMembers")
        .field(Field::new(
            "rpslPk",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let row = ctx.parent_value.try_downcast_ref::<SetMembers>()?;
                    Ok(Some(FieldValue::value(row.rpsl_pk.clone())))
                })
            },
        ))
        .field(Field::new(
            "rootSource",
            TypeRef::named_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let row = ctx.parent_value.try_downcast_ref::<SetMembers>()?;
                    Ok(Some(FieldValue::value(row.root_source.clone())))
                })
            },
        ))
        .field(Field::new(
            "members",
            TypeRef::named_nn_list(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let row = ctx.parent_value.try_downcast_ref::<SetMembers>()?;
                    Ok(Some(FieldValue::list(
                        row.members.iter().map(FieldValue::value),
                    )))
                })
            },
        ))
}

async fn recursive_set_members(
    ctx: ResolverContext<'_>,
) -> async_graphql::Result<Option<FieldValue<'_>>> {
    let lookup = ctx.data::<Lookup>()?;
    let names = set_names(&ctx)?;
    let mut sources = source_filter(&ctx, &lookup.sources)?;
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(source.clone()));
    let root_source = ctx
        .args
        .get("rootSource")
        .filter(|value| !value.is_null())
        .map(|value| value.string().map(|name| name.trim().to_ascii_uppercase()))
        .transpose()?;
    if root_source
        .as_ref()
        .is_some_and(|source| !lookup.sources.contains(source))
    {
        return Err(Error::new("Unknown rootSource."));
    }
    let depth = ctx
        .args
        .get("depth")
        .filter(|value| !value.is_null())
        .map(|value| value.i64())
        .transpose()?
        .unwrap_or(0);
    let depth = usize::try_from(depth)
        .ok()
        .filter(|&depth| depth <= sets::MAX_DEPTH)
        .ok_or_else(|| Error::new(format!("depth must be between 0 and {}.", sets::MAX_DEPTH)))?;
    let depth = (depth != 0).then_some(depth);
    let excluded = list_filter(&ctx, "excludeSets")?.unwrap_or_default();
    let rows = lookup
        .run(move |store| {
            let root_sources = root_source
                .as_ref()
                .map_or(sources.as_slice(), std::slice::from_ref);
            let mut rows = Vec::new();
            let mut members_remaining = MAX_SET_MEMBERS;
            let mut bytes_remaining = MAX_SET_MEMBER_BYTES;
            for name in names {
                for source in root_sources {
                    let members = recursive_members_at_root(
                        store, &sources, source, &name, depth, &excluded,
                    )?;
                    if members.is_none() && root_source.is_none() {
                        continue;
                    }
                    let members = members.unwrap_or_default();
                    let bytes = members.iter().map(|member| member.len() + 1).sum::<usize>();
                    if rows.len() == MAX_SET_ROOTS
                        || members.len() > members_remaining
                        || bytes > bytes_remaining
                    {
                        return Ok(None);
                    }
                    members_remaining -= members.len();
                    bytes_remaining -= bytes;
                    rows.push(SetMembers {
                        rpsl_pk: name.clone(),
                        root_source: source.clone(),
                        members,
                    });
                }
            }
            Ok(Some(rows))
        })
        .await?
        .ok_or_else(|| Error::new("Set expansion result exceeds GraphQL limits."))?;
    Ok(Some(FieldValue::list(
        rows.into_iter().map(FieldValue::owned_any),
    )))
}

struct DatabaseStatus {
    source: String,
    authoritative: bool,
    serial: Option<u64>,
    local_serial: Option<u64>,
    journal: Option<(u64, u64)>,
    nrtm4: Option<(String, u64)>,
}

fn status_int(serial: Option<u64>) -> Option<Value> {
    // The upstream fields use GraphQL's signed 32-bit Int. Preserve the
    // original cursor in storage and omit values the wire type cannot carry.
    serial
        .and_then(|serial| i32::try_from(serial).ok())
        .map(Value::from)
}

fn database_status_type() -> GraphqlObject {
    let string = TypeRef::named_nn(TypeRef::STRING);
    let boolean = TypeRef::named_nn(TypeRef::BOOLEAN);
    let int = TypeRef::named(TypeRef::INT);
    type StatusValue = fn(&DatabaseStatus) -> Option<Value>;
    let fields: [(&str, TypeRef, StatusValue); 12] = [
        ("source_type", string.clone(), |_| {
            Some(Value::from("regular"))
        }),
        ("source", string, |status| {
            Some(Value::from(status.source.as_str()))
        }),
        ("authoritative", boolean.clone(), |status| {
            Some(Value::from(status.authoritative))
        }),
        ("rpkiRovFilter", boolean.clone(), |_| {
            Some(Value::from(false))
        }),
        ("scopefilterEnabled", boolean.clone(), |_| {
            Some(Value::from(false))
        }),
        ("localJournalKept", boolean.clone(), |status| {
            Some(Value::from(
                status.local_serial.is_some() || status.journal.is_some(),
            ))
        }),
        ("synchronisedSerials", boolean, |status| {
            let newest = status.journal.map(|(_, newest)| newest);
            Some(Value::from(newest.is_some() && newest == status.serial))
        }),
        ("serialOldestJournal", int.clone(), |status| {
            status_int(status.journal.map(|(oldest, _)| oldest))
        }),
        ("serialNewestJournal", int.clone(), |status| {
            status_int(status.journal.map(|(_, newest)| newest))
        }),
        ("serialNewestMirror", int.clone(), |status| {
            status_int(status.serial)
        }),
        (
            "nrtm4ClientSessionId",
            TypeRef::named(TypeRef::STRING),
            |status| {
                status
                    .nrtm4
                    .as_ref()
                    .map(|(session, _)| Value::from(session.as_str()))
            },
        ),
        ("nrtm4ClientVersion", int.clone(), |status| {
            status_int(status.nrtm4.as_ref().map(|(_, version)| *version))
        }),
    ];
    let mut object = GraphqlObject::new("DatabaseStatus");
    for (name, ty, value) in fields {
        object = object.field(Field::new(name, ty, move |ctx| {
            FieldFuture::new(async move {
                let status = ctx.parent_value.try_downcast_ref::<DatabaseStatus>()?;
                Ok(value(status).map(FieldValue::value))
            })
        }));
    }
    for (name, ty) in [
        ("objectClassFilter", TypeRef::named_nn_list(TypeRef::STRING)),
        ("routePreference", int.clone()),
        ("serialLastExport", int.clone()),
        ("nrtm4ServerSessionId", TypeRef::named(TypeRef::STRING)),
        ("nrtm4ServerVersion", int.clone()),
        (
            "nrtm4ServerLastUpdateNotificationFileUpdate",
            TypeRef::named(TypeRef::STRING),
        ),
        ("nrtm4ServerLastSnapshotVersion", int),
        ("rpslDataUpdated", TypeRef::named(TypeRef::STRING)),
        ("lastUpdate", TypeRef::named(TypeRef::STRING)),
        ("aliased_sources", TypeRef::named_nn_list(TypeRef::STRING)),
    ] {
        object = object.field(Field::new(name, ty, |_| {
            FieldFuture::new(async { Ok(None::<FieldValue<'_>>) })
        }));
    }
    object
}

async fn database_status(
    ctx: ResolverContext<'_>,
) -> async_graphql::Result<Option<FieldValue<'_>>> {
    let lookup = ctx.data::<Lookup>()?;
    let mut sources = source_filter(&ctx, &lookup.sources)?;
    if sources.len() > MAX_FILTER_VALUES {
        return Err(Error::new(format!(
            "Select at most {MAX_FILTER_VALUES} sources for databaseStatus."
        )));
    }
    if sources.is_empty() {
        return Ok(Some(FieldValue::list(Vec::<FieldValue<'_>>::new())));
    }
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(source.clone()));
    let authoritative_sources = Arc::clone(&lookup.authoritative_sources);
    let rows = lookup
        .run(move |store| {
            sources
                .into_iter()
                .map(|source| {
                    let authoritative = authoritative_sources.contains(&source);
                    let serial = store.source_serial(&source)?;
                    let local_serial = store.source_local_serial(&source)?;
                    let journal = store.nrtm3_journal_bounds(&source)?;
                    let nrtm4 = store
                        .nrtm4_state(&source)?
                        .map(|state| (state.session_id, state.version));
                    Ok(DatabaseStatus {
                        source,
                        authoritative,
                        serial,
                        local_serial,
                        journal,
                        nrtm4,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .await?;
    Ok(Some(FieldValue::list(
        rows.into_iter().map(FieldValue::owned_any),
    )))
}

fn make_schema(
    store: Arc<Store>,
    sources: Vec<String>,
    authoritative_sources: HashSet<String>,
) -> Result<Schema> {
    let list = TypeRef::named_nn_list(TypeRef::STRING);
    let ip_version = TypeRef::named(TypeRef::INT);
    let root = GraphqlObject::new("Query")
        .field(
            Field::new(
                "rpslObjects",
                TypeRef::named_nn_list(OBJECT_INTERFACE),
                |ctx| FieldFuture::new(objects(ctx)),
            )
            .argument(InputValue::new("rpslPk", list.clone()))
            .argument(InputValue::new("objectClass", list.clone()))
            .argument(InputValue::new("sources", list.clone()))
            .argument(InputValue::new("recordLimit", TypeRef::named(TypeRef::INT))),
        )
        .field(
            Field::new(
                "databaseStatus",
                TypeRef::named_list("DatabaseStatus"),
                |ctx| FieldFuture::new(database_status(ctx)),
            )
            .argument(InputValue::new("sources", list.clone())),
        )
        .field(
            Field::new(
                "asnPrefixes",
                TypeRef::named_nn_list("ASNPrefixes"),
                |ctx| FieldFuture::new(asn_prefixes(ctx)),
            )
            .argument(InputValue::new("asns", TypeRef::named_nn_list_nn("ASN")))
            .argument(InputValue::new("ipVersion", ip_version.clone()))
            .argument(InputValue::new("sources", list.clone())),
        )
        .field(
            Field::new(
                "asSetPrefixes",
                TypeRef::named_nn_list("AsSetPrefixes"),
                |ctx| FieldFuture::new(as_set_prefixes(ctx)),
            )
            .argument(InputValue::new(
                "setNames",
                TypeRef::named_nn_list_nn(TypeRef::STRING),
            ))
            .argument(InputValue::new("ipVersion", ip_version))
            .argument(InputValue::new("sources", list.clone()))
            .argument(InputValue::new("excludeSets", list.clone()))
            .argument(InputValue::new(
                "sqlTrace",
                TypeRef::named(TypeRef::BOOLEAN),
            )),
        )
        .field(
            Field::new(
                "recursiveSetMembers",
                TypeRef::named_nn_list("SetMembers"),
                |ctx| FieldFuture::new(recursive_set_members(ctx)),
            )
            .argument(InputValue::new(
                "setNames",
                TypeRef::named_nn_list_nn(TypeRef::STRING),
            ))
            .argument(InputValue::new("depth", TypeRef::named(TypeRef::INT)))
            .argument(InputValue::new(
                "rootSource",
                TypeRef::named(TypeRef::STRING),
            ))
            .argument(InputValue::new("sources", list.clone()))
            .argument(InputValue::new("excludeSets", list))
            .argument(InputValue::new(
                "sqlTrace",
                TypeRef::named(TypeRef::BOOLEAN),
            )),
        );
    let mut interface = Interface::new(OBJECT_INTERFACE);
    for &(name, output) in COMMON_FIELDS {
        interface = interface.field(InterfaceField::new(name, output.graphql_type()));
    }

    let mut builder = Schema::build("Query", None, None)
        .data(Lookup {
            store,
            sources: sources.into(),
            authoritative_sources: Arc::new(authoritative_sources),
            slots: Arc::new(Semaphore::new(MAX_LOOKUPS)),
        })
        .limit_depth(MAX_DEPTH)
        .limit_complexity(MAX_COMPLEXITY)
        .limit_recursive_depth(MAX_DEPTH + 4)
        .limit_directives(4)
        .register(root)
        .register(database_status_type())
        .register(interface)
        .register(Scalar::new("ASN").validator(|value| asn_value(value).is_some()))
        .register(Scalar::new("IP").validator(|value| matches!(value, Value::String(_))))
        .register(prefix_type("ASNPrefixes", "asn", "ASN"))
        .register(prefix_type("AsSetPrefixes", "rpslPk", TypeRef::STRING))
        .register(set_members_type());

    for &(class, name) in OBJECT_TYPES {
        let mut object = GraphqlObject::new(name).implement(OBJECT_INTERFACE);
        for &(field, output) in COMMON_FIELDS.iter().chain(class_fields(class)) {
            object = object.field(output.field(field));
        }
        if class != "aut-num" {
            let output = match class {
                "route" | "route6" => Output::RoutePrefix,
                "inet-rtr" => Output::Key,
                _ => Output::First(class),
            };
            object = object.field(output.field(camel_case(class)));
        }
        builder = builder.register(object);
    }
    builder.finish().context("building GraphQL schema")
}

// HTTP is loopback-only. Reject arbitrary DNS names before any route (including
// updates) to prevent a browser reaching the private API through DNS rebinding.
fn allowed_authority(value: &str, listen_ip: IpAddr) -> bool {
    if value.contains('@') {
        return false;
    }
    let Ok(authority) = value.parse::<Authority>() else {
        return false;
    };
    let Some(suffix) = authority.as_str().strip_prefix(authority.host()) else {
        return false;
    };
    if !suffix.is_empty()
        && !suffix.strip_prefix(':').is_some_and(|port| {
            port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
        })
    {
        return false;
    }
    let host = authority.host();
    let host = host
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().ok() == Some(listen_ip)
}

async fn bound_host(State(listen_ip): State<IpAddr>, req: HttpRequest, next: Next) -> Response {
    let mut hosts = req.headers().get_all(HOST).iter();
    let valid = hosts
        .next()
        .and_then(|h| h.to_str().ok())
        .is_some_and(|host| allowed_authority(host, listen_ip))
        && hosts.next().is_none()
        && req
            .uri()
            .authority()
            .is_none_or(|authority| allowed_authority(authority.as_str(), listen_ip));
    if !valid {
        return (StatusCode::BAD_REQUEST, "Invalid host header").into_response();
    }
    next.run(req).await
}

async fn bound_http(State(limits): State<HttpLimits>, req: HttpRequest, next: Next) -> Response {
    let Ok(_permit) = limits.slots.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match tokio::time::timeout(limits.timeout, next.run(req)).await {
        Ok(response) => response,
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}

async fn execute(
    State(schema): State<Schema>,
    Extension(store): Extension<Arc<Store>>,
    Json(request): Json<Request>,
) -> impl IntoResponse {
    let _view = store.query_gate().read().await;
    Json(schema.execute(request).await)
}

pub struct HttpServiceConfig {
    pub sources: Vec<String>,
    pub authoritative_sources: HashSet<String>,
    pub update_sources: Option<HashSet<String>>,
    pub timeout: Duration,
    pub dashboard: crate::dashboard::Metadata,
    pub rpki: Arc<crate::rpki_service::Shared>,
    pub whois_sources: Option<Vec<String>>,
}

pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    config: HttpServiceConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let HttpServiceConfig {
        sources,
        authoritative_sources,
        update_sources,
        timeout,
        dashboard,
        rpki,
        whois_sources,
    } = config;
    let schema = make_schema(store.clone(), sources.clone(), authoritative_sources)?;
    let mut app = Router::new()
        .route("/graphql", post(execute))
        .route("/graphql/", post(execute))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BYTES))
        .layer(middleware::from_fn_with_state(
            HttpLimits {
                slots: Arc::new(Semaphore::new(MAX_HTTP_REQUESTS)),
                timeout,
            },
            bound_http,
        ))
        .with_state(schema)
        .layer(Extension(store.clone()))
        .merge(crate::whois::http_router(
            store.clone(),
            whois_sources.unwrap_or_else(|| sources.clone()),
            timeout,
        ));
    if let Some(update_sources) = update_sources {
        app = app.merge(crate::submission_http::router(
            store.clone(),
            update_sources,
            timeout,
        ));
    }
    app = app.merge(crate::dashboard::router(dashboard, store.clone())?);
    app = app.merge(crate::rpki_service::router(rpki));
    app = app.merge(crate::irr_api::router(store.clone(), sources, timeout));
    app = app.layer(middleware::from_fn_with_state(
        listener.local_addr()?.ip(),
        bound_host,
    ));
    tracing::info!(address = %listener.local_addr()?, "GraphQL listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("serving GraphQL")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value as JsonValue, json};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::oneshot,
    };

    use super::*;

    fn test_http_config(
        address: std::net::SocketAddr,
        update_sources: Option<HashSet<String>>,
    ) -> HttpServiceConfig {
        HttpServiceConfig {
            sources: vec!["TEST".into()],
            authoritative_sources: HashSet::new(),
            update_sources,
            timeout: Duration::from_secs(2),
            whois_sources: None,
            rpki: crate::rpki_service::Shared::new(None),
            dashboard: crate::dashboard::Metadata {
                mode: "live",
                version: env!("CARGO_PKG_VERSION"),
                node: Default::default(),
                sources: vec!["TEST".into()],
                whois_listen: address.to_string(),
                max_connections: 8,
                database_cache_mib: 64,
            },
        }
    }

    fn test_schema() -> Result<(tempfile::TempDir, Schema)> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("graphql.redb"))?);
        let record = |text| crate::rpsl::parse(text).map_err(anyhow::Error::msg);
        store.replace_source(
            "TEST",
            [
                record("mntner: TEST-MNT\nauth: BCRYPT-PW hidden-secret\n+ folded-secret\nsource: TEST"),
                record(
                    "aut-num: AS64496\nas-name: EXAMPLE\nmnt-by: test-mnt, extra-mnt\nsource: TEST",
                ),
                record("route6: 2001:DB8:0::/32\norigin: as064496\nsource: TEST"),
                record("route: 192.0.2.0/24\norigin: AS64496\nsource: TEST"),
                record("route: 198.51.100.0/24\norigin: AS4200000000\nsource: TEST"),
                record("as-set: AS-EXAMPLE\nmembers: AS64496, AS-NESTED\nsource: TEST"),
                record("as-set: AS-NESTED\nmembers: AS4200000000\nsource: TEST"),
                record("as-set: AS-DUPLICATE\nmembers: AS64496,AS-CROSS\nsource: TEST"),
                record("as-set: AS-CROSS\nmembers: AS64500\nsource: TEST"),
                record("filter-set: AS64496:FLTR-DEMO\nfilter: AS64496\nmp-filter: afi ipv6 ANY\nsource: TEST"),
                record("peering-set: AS64496:PRNG-DEMO\npeering: AS64496\npeering: AS64497\nsource: TEST"),
                record("rtr-set: AS64496:RTRS-DEMO\nmembers: r1.example\nmp-members: AS64496:RTRS-CHILD\nmbrs-by-ref: ANY\nsource: TEST"),
                record("inet-rtr: r1.example\nlocal-as: as064496\nalias: alias.example\nmember-of: AS64496:RTRS-DEMO\nsource: TEST"),
            ],
        )?;
        store.replace_source(
            "OTHER",
            [
                record("aut-num: AS64496\nas-name: OTHER\nsource: OTHER"),
                record("route: 192.0.2.0/24\norigin: AS64496\nsource: OTHER"),
                record("route: 203.0.113.0/24\norigin: AS64496\nsource: OTHER"),
                record("as-set: AS-DUPLICATE\nmembers: AS64497,AS-CROSS\nsource: OTHER"),
                record("as-set: AS-CROSS\nmembers: AS64501\nsource: OTHER"),
            ],
        )?;
        let schema = make_schema(store, vec!["TEST".into(), "OTHER".into()], HashSet::new())?;
        Ok((directory, schema))
    }

    #[tokio::test]
    async fn database_status_reports_config_and_committed_mirror_state() -> Result<()> {
        use crate::{
            nrtm3::{Kind, Operation},
            nrtm4_records::Record,
            nrtm4_state::fixture_state,
        };

        const SESSION: &str = "c86f0f62-447e-4bdf-8cb6-18f861bc3532";
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("status.redb"))?);
        let writer = Arc::clone(&store);
        spawn_blocking(move || -> Result<()> {
            let record = |text| crate::rpsl::parse(text).map_err(anyhow::Error::msg);
            writer.replace_source("MIRROR", [record("aut-num: AS64496\nsource: MIRROR")])?;
            writer.apply_nrtm3(
                "MIRROR",
                (42, 42),
                Some(41),
                [Ok(Operation {
                    kind: Kind::Add,
                    serial: 42,
                    object: record("aut-num: AS64497\nsource: MIRROR")?,
                })],
            )?;
            let state = fixture_state("NRTMFOUR", SESSION, 7);
            writer.apply_nrtm4(
                "NRTMFOUR",
                None,
                &state,
                true,
                [Ok(Record::Snapshot(record(
                    "aut-num: AS64496\nsource: NRTMFOUR",
                )?))],
            )?;
            Ok(())
        })
        .await??;
        let schema = make_schema(
            store,
            vec!["TEST".into(), "MIRROR".into(), "NRTMFOUR".into()],
            HashSet::from(["TEST".into()]),
        )?;
        let response = schema
            .execute(
                r#"{
                    databaseStatus(sources:["nrtmfour","TEST","MIRROR","MIRROR"]) {
                        source_type source authoritative rpkiRovFilter scopefilterEnabled
                        localJournalKept synchronisedSerials serialNewestMirror
                        serialOldestJournal serialNewestJournal
                        nrtm4ClientSessionId nrtm4ClientVersion nrtm4ServerVersion aliased_sources
                    }
                }"#,
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({"databaseStatus":[
                {
                    "source_type":"regular","source":"NRTMFOUR","authoritative":false,
                    "rpkiRovFilter":false,"scopefilterEnabled":false,
                    "localJournalKept":false,"synchronisedSerials":false,"serialNewestMirror":null,
                    "serialOldestJournal":null,"serialNewestJournal":null,
                    "nrtm4ClientSessionId":SESSION,"nrtm4ClientVersion":7,
                    "nrtm4ServerVersion":null,"aliased_sources":null
                },
                {
                    "source_type":"regular","source":"TEST","authoritative":true,
                    "rpkiRovFilter":false,"scopefilterEnabled":false,
                    "localJournalKept":false,"synchronisedSerials":false,"serialNewestMirror":null,
                    "serialOldestJournal":null,"serialNewestJournal":null,
                    "nrtm4ClientSessionId":null,"nrtm4ClientVersion":null,
                    "nrtm4ServerVersion":null,"aliased_sources":null
                },
                {
                    "source_type":"regular","source":"MIRROR","authoritative":false,
                    "rpkiRovFilter":false,"scopefilterEnabled":false,
                    "localJournalKept":true,"synchronisedSerials":true,"serialNewestMirror":42,
                    "serialOldestJournal":42,"serialNewestJournal":42,
                    "nrtm4ClientSessionId":null,"nrtm4ClientVersion":null,
                    "nrtm4ServerVersion":null,"aliased_sources":null
                }
            ]})
        );
        Ok(())
    }

    #[tokio::test]
    async fn database_status_bounds_default_selection_and_keeps_pinned_int_type() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("status.redb"))?);
        let sources = (0..=MAX_FILTER_VALUES)
            .map(|index| format!("SOURCE{index}"))
            .collect();
        let schema = make_schema(store, sources, HashSet::new())?;
        assert!(
            schema
                .sdl()
                .contains("databaseStatus(sources: [String!]): [DatabaseStatus]")
        );
        assert_eq!(status_int(Some(i32::MAX as u64 + 1)), None);
        assert_eq!(
            status_int(Some(i32::MAX as u64)),
            Some(Value::from(i32::MAX))
        );
        for invalid in [
            "{ databaseStatus { source } }",
            r#"{ databaseStatus(sources:["UNKNOWN"]) { source } }"#,
        ] {
            assert!(
                !schema.execute(invalid).await.errors.is_empty(),
                "{invalid}"
            );
        }
        let response = schema
            .execute(r#"{ databaseStatus(sources:["SOURCE0"]) { source } }"#)
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({"databaseStatus":[{"source":"SOURCE0"}]})
        );
        Ok(())
    }

    #[tokio::test]
    async fn recursive_set_members_separates_root_and_nested_source_order() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let response = schema
            .execute(r#"{
                recursiveSetMembers(setNames:["as-duplicate","AS-DUPLICATE","AS-MISSING"], sources:["OTHER","TEST"]) {
                    rpslPk rootSource members
                }
            }"#)
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({"recursiveSetMembers":[
                {"rpslPk":"AS-DUPLICATE","rootSource":"OTHER","members":["AS64497","AS64501"]},
                {"rpslPk":"AS-DUPLICATE","rootSource":"TEST","members":["AS64496","AS64501"]}
            ]})
        );

        let response = schema
            .execute(r#"{
                recursiveSetMembers(setNames:["AS-DUPLICATE","AS-MISSING"], rootSource:"test", sources:["OTHER"]) {
                    rpslPk rootSource members
                }
            }"#)
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({"recursiveSetMembers":[
                {"rpslPk":"AS-DUPLICATE","rootSource":"TEST","members":["AS64496","AS64501"]},
                {"rpslPk":"AS-MISSING","rootSource":"TEST","members":[]}
            ]})
        );
        Ok(())
    }

    #[tokio::test]
    async fn recursive_set_members_supports_depth_and_excludes_with_input_limits() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let response = schema
            .execute(r#"{
                limited: recursiveSetMembers(setNames:["AS-DUPLICATE"], rootSource:"TEST", depth:1) { members }
                excluded: recursiveSetMembers(setNames:["AS-DUPLICATE"], rootSource:"TEST", excludeSets:["as-cross"]) { members }
            }"#)
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({
                "limited":[{"members":["AS-CROSS","AS64496"]}],
                "excluded":[{"members":["AS64496"]}]
            })
        );
        for query in [
            r#"{ recursiveSetMembers(setNames:["AS-DUPLICATE"], rootSource:"UNKNOWN") { members } }"#,
            r#"{ recursiveSetMembers(setNames:["AS-DUPLICATE"], depth:-1) { members } }"#,
            r#"{ recursiveSetMembers(setNames:["AS-DUPLICATE"], depth:65) { members } }"#,
            r#"{ recursiveSetMembers(setNames:["AS-DUPLICATE"], sqlTrace:true) { members } }"#,
        ] {
            assert!(!schema.execute(query).await.errors.is_empty(), "{query}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn exposes_policy_and_router_fields_with_canonical_router_identity() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let response = schema.execute(r#"{
            filter: rpslObjects(rpslPk:["AS64496:FLTR-DEMO"],sources:["TEST"]) { ... on RPSLFilterSet { filter mpFilter } }
            peering: rpslObjects(rpslPk:["AS64496:PRNG-DEMO"],sources:["TEST"]) { ... on RPSLPeeringSet { peering mpPeering } }
            routers: rpslObjects(rpslPk:["AS64496:RTRS-DEMO"],sources:["TEST"]) { ... on RPSLRtrSet { members mpMembers mbrsByRef } }
            router: rpslObjects(rpslPk:["r1.example"],sources:["TEST"]) { ... on RPSLInetRtr { inetRtr localAs alias memberOf } }
        }"#).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?,
            json!({
                "filter":[{"filter":"AS64496","mpFilter":"afi ipv6 ANY"}],
                "peering":[{"peering":["AS64496","AS64497"],"mpPeering":null}],
                "routers":[{"members":["R1.EXAMPLE"],"mpMembers":["AS64496:RTRS-CHILD"],"mbrsByRef":["ANY"]}],
                "router":[{"inetRtr":"R1.EXAMPLE","localAs":"AS64496","alias":["ALIAS.EXAMPLE"],"memberOf":["AS64496:RTRS-DEMO"]}]
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn queries_exact_keys_filters_and_typed_fields() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let request: Request = serde_json::from_value(json!({
            "query": "query Find($keys:[String!]) {
                rpslObjects(rpslPk:$keys, sources:[\"test\"], objectClass:[\"aut-num\"], recordLimit:1) {
                    __typename rpslPk objectClass source mntBy ... on RPSLAutNum { autNum asName }
                }
            }",
            "variables": {"keys": ["TEST-MNT", "as64496", "AS64497"]}
        }))?;
        let response = schema.execute(request).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            serde_json::to_value(response.data)?,
            json!({"rpslObjects":[{
                "__typename":"RPSLAutNum",
                "rpslPk":"AS64496",
                "objectClass":"aut-num",
                "source":"TEST",
                "mntBy":["TEST-MNT", "EXTRA-MNT"],
                "autNum":"AS64496",
                "asName":"EXAMPLE",
            }]})
        );
        let route = schema
            .execute(
                r#"{ rpslObjects(rpslPk:["2001:db8::/32AS64496"]) {
                    ... on RPSLRoute6 { route6 origin }
                } }"#,
            )
            .await;
        assert!(route.errors.is_empty(), "{:?}", route.errors);
        assert_eq!(
            serde_json::to_value(route.data)?,
            json!({"rpslObjects":[{"route6":"2001:db8::/32","origin":"AS64496"}]})
        );
        Ok(())
    }

    #[tokio::test]
    async fn exposes_only_public_object_text_and_returns_empty_hits() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let response = schema
            .execute(
                r#"{
                    hit: rpslObjects(rpslPk:["TEST-MNT"]) { objectText }
                    miss: rpslObjects(rpslPk:["AS64497"]) { rpslPk }
                }"#,
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data: JsonValue = serde_json::to_value(response.data)?;
        assert_eq!(
            data["hit"][0]["objectText"],
            "mntner: TEST-MNT\nsource: TEST"
        );
        assert_eq!(data["miss"], json!([]));
        Ok(())
    }

    #[tokio::test]
    async fn resolves_asn_prefixes_with_unsigned_asns_and_source_filters() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let request: Request = serde_json::from_value(json!({
            "query": "query Find($origins:[ASN!]!) {
                all: asnPrefixes(asns:$origins) { asn prefixes }
                ipv6: asnPrefixes(asns:[64496], ipVersion:6, sources:[\"TEST\"]) { asn prefixes }
                filtered: asnPrefixes(asns:[64496], ipVersion:4, sources:[\"OTHER\"]) { asn prefixes }
            }",
            "variables": {"origins": [4_200_000_000_u32, 64496, 64497, "64496"]}
        }))?;
        let response = schema.execute(request).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let both = json!({
            "asn":64496,
            "prefixes":["192.0.2.0/24","203.0.113.0/24","2001:db8::/32"]
        });
        assert_eq!(
            serde_json::to_value(response.data)?,
            json!({
                "all": [
                    {"asn":4_200_000_000_u32, "prefixes":["198.51.100.0/24"]},
                    both,
                    {"asn":64497, "prefixes":[]},
                    both
                ],
                "ipv6":[{"asn":64496, "prefixes":["2001:db8::/32"]}],
                "filtered":[{"asn":64496, "prefixes":["192.0.2.0/24","203.0.113.0/24"]}]
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn expands_as_set_prefixes_with_exclusions_and_empty_results() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        let response = schema
            .execute(
                r#"{
                    expanded: asSetPrefixes(setNames:["as-example","AS-EXAMPLE","AS-MISSING"], sources:["TEST"]) {
                        rpslPk prefixes
                    }
                    excluded: asSetPrefixes(setNames:["AS-EXAMPLE"], excludeSets:["as-nested"], ipVersion:4) {
                        rpslPk prefixes
                    }
                    filtered: asSetPrefixes(setNames:["AS-EXAMPLE"], sources:["OTHER"]) { rpslPk prefixes }
                }"#,
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            serde_json::to_value(response.data)?,
            json!({
                "expanded":[
                    {"rpslPk":"AS-EXAMPLE","prefixes":["192.0.2.0/24","198.51.100.0/24","2001:db8::/32"]},
                    {"rpslPk":"AS-MISSING","prefixes":[]}
                ],
                "excluded":[{"rpslPk":"AS-EXAMPLE","prefixes":["192.0.2.0/24","203.0.113.0/24"]}],
                "filtered":[{"rpslPk":"AS-EXAMPLE","prefixes":[]}]
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unbounded_queries_and_large_alias_sets() -> Result<()> {
        let (_directory, schema) = test_schema()?;
        for query in [
            r#"{ rpslObjects { rpslPk } }"#,
            r#"{ rpslObjects(rpslPk:["AS64496"], recordLimit:9) { rpslPk } }"#,
            r#"{ rpslObjects(rpslPk:["AS64496"], sources:["UNKNOWN"]) { rpslPk } }"#,
            r#"{ asnPrefixes(asns:[4294967296]) { prefixes } }"#,
            r#"{ asnPrefixes(asns:[64496], ipVersion:5) { prefixes } }"#,
            r#"{ asSetPrefixes(setNames:["AS-EXAMPLE"], sqlTrace:true) { prefixes } }"#,
            r#"{ asSetPrefixes(setNames:["AS-EXAMPLE"], sources:["UNKNOWN"]) { prefixes } }"#,
        ] {
            assert!(!schema.execute(query).await.errors.is_empty(), "{query}");
        }

        let aliases = (0..MAX_COMPLEXITY)
            .map(|index| format!(r#"q{index}: rpslObjects(rpslPk:["AS64496"]) {{ rpslPk }}"#))
            .collect::<Vec<_>>()
            .join(" ");
        let response = schema
            .execute(format!(
                "query {{ ...AliasSet }} fragment AliasSet on Query {{ {aliases} }}"
            ))
            .await;
        assert!(
            response
                .errors
                .iter()
                .any(|error| error.message.contains("complex")),
            "{:?}",
            response.errors
        );
        Ok(())
    }

    #[test]
    fn loopback_authorities_reject_external_and_ambiguous_names() {
        let ipv4: IpAddr = "127.0.0.1".parse().unwrap();
        let ipv6: IpAddr = "::1".parse().unwrap();
        for host in ["localhost", "LOCALHOST:9080", "127.0.0.1", "127.0.0.1:9080"] {
            assert!(allowed_authority(host, ipv4), "{host}");
        }
        assert!(allowed_authority("[::1]:9080", ipv6));
        for host in [
            "",
            "evil.example",
            "localhost.evil.example",
            "evil@localhost",
            "localhost,evil.example",
            "localhost:99999",
            "localhost:bad",
            "localhost:+80",
            "localhost.",
            "[::1]",
            "127.0.0.2",
            "localhost/path",
        ] {
            assert!(!allowed_authority(host, ipv4), "{host}");
        }
    }

    #[tokio::test]
    async fn all_http_routes_reject_untrusted_hosts_before_handling_requests() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("host.redb"))?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve(
            listener,
            store,
            test_http_config(address, Some(HashSet::from(["TEST".into()]))),
            async move {
                let _ = stopped.await;
            },
        ));
        async fn request(
            address: std::net::SocketAddr,
            target: &str,
            host: &str,
        ) -> Result<String> {
            let mut connection = TcpStream::connect(address).await?;
            let body = r#"{"query":"{ rpslObjects(rpslPk:[\"AS64496\"]) { rpslPk } }"}"#;
            connection.write_all(format!("POST {target} HTTP/1.0\r\n{host}Connection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await?;
            let mut reply = String::new();
            tokio::time::timeout(
                Duration::from_secs(3),
                connection.read_to_string(&mut reply),
            )
            .await??;
            Ok(reply)
        }
        for target in [
            "/graphql/",
            "/v1/whois/?q=AS64496",
            "/v1/submit/",
            "/missing",
        ] {
            for host in [
                "Host: secret.evil.example\r\n",
                "",
                "Host: localhost\r\nHost: secret.evil.example\r\n",
            ] {
                let reply = request(address, target, host).await?;
                assert_eq!(
                    reply.split_whitespace().nth(1),
                    Some("400"),
                    "{target}: {reply}"
                );
                assert!(!reply.contains("secret.evil.example"));
            }
        }
        let reply = request(
            address,
            "http://secret.evil.example/graphql/",
            "Host: localhost\r\n",
        )
        .await?;
        assert_eq!(reply.split_whitespace().nth(1), Some("400"), "{reply}");
        for host in [
            "Host: localhost\r\n".to_owned(),
            format!("Host: {address}\r\n"),
        ] {
            let reply = request(address, "/graphql/", &host).await?;
            assert_eq!(reply.split_whitespace().nth(1), Some("200"), "{reply}");
        }
        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }

    #[tokio::test]
    async fn accepts_json_posts_and_limits_request_bytes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(&directory.path().join("http.redb"))?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve(
            listener,
            store,
            test_http_config(address, None),
            async move {
                let _ = stopped.await;
            },
        ));

        async fn request(address: std::net::SocketAddr, body: &str) -> Result<String> {
            let mut connection = TcpStream::connect(address).await?;
            let headers = format!(
                "POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            connection
                .write_all(format!("{headers}{body}").as_bytes())
                .await?;
            let mut reply = String::new();
            tokio::time::timeout(
                Duration::from_secs(3),
                connection.read_to_string(&mut reply),
            )
            .await??;
            Ok(reply)
        }

        let body = json!({"query":"{ rpslObjects(rpslPk:[\"AS64496\"]) { rpslPk } }"}).to_string();
        let reply = request(address, &body).await?;
        assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
        assert!(reply.contains(r#""rpslObjects":[]"#), "{reply}");
        let reply = request(address, &" ".repeat(MAX_HTTP_BYTES + 1)).await?;
        assert!(reply.starts_with("HTTP/1.1 413 "), "{reply}");

        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }
}
