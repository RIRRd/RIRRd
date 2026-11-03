//! Loopback-only, response-validating load client for the synthetic PERF fixture.
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, ValueEnum};
use serde_json::json;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

const MAX_BODY: usize = 1024 * 1024;
const MAX_SAMPLES_PER_CLIENT: usize = 2_000_000;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Whois,
    Graphql,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Connections {
    Reused,
    New,
}
#[derive(Clone, Parser)]
struct Options {
    #[arg(long)]
    endpoint: SocketAddr,
    #[arg(long, value_enum)]
    mode: Mode,
    #[arg(long, value_enum, default_value = "reused")]
    connections: Connections,
    #[arg(long, default_value_t = 8)]
    clients: usize,
    #[arg(long, default_value_t = 20)]
    seconds: u64,
    #[arg(long, default_value_t = 3)]
    warmup: u64,
    #[arg(long, default_value_t = 50000)]
    keys: usize,
}

async fn line<R: AsyncBufRead + Unpin>(input: &mut R, cap: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    (&mut *input)
        .take((cap + 1) as u64)
        .read_until(b'\n', &mut data)
        .await?;
    ensure!(
        data.len() <= cap && data.ends_with(b"\n"),
        "invalid or oversized response line"
    );
    Ok(data)
}

async fn whois_body<R: AsyncBufRead + Unpin>(input: &mut R, key: &str) -> Result<usize> {
    let header = line(input, 128).await?;
    let header = std::str::from_utf8(&header)?;
    let len: usize = header
        .strip_prefix('A')
        .context("WHOIS did not return success")?
        .trim()
        .parse()?;
    ensure!((1..=MAX_BODY).contains(&len), "invalid WHOIS byte count");
    let mut data = vec![0; len];
    input.read_exact(&mut data).await?;
    let mut end = [0; 2];
    input.read_exact(&mut end).await?;
    ensure!(
        end == *b"C\n" && data.ends_with(b"\n"),
        "invalid WHOIS trailer"
    );
    let text = std::str::from_utf8(&data)?;
    let field = |name: &str, expected: &str| {
        text.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(n, v)| n == name && v.trim() == expected)
        })
    };
    ensure!(
        field("aut-num", key) && field("source", "PERF"),
        "incorrect WHOIS object identity"
    );
    Ok(data.len())
}

async fn http_body<R: AsyncBufRead + Unpin>(input: &mut R) -> Result<(Vec<u8>, bool)> {
    let status = line(input, 4096).await?;
    ensure!(
        std::str::from_utf8(&status)?.split_whitespace().nth(1) == Some("200"),
        "HTTP status was not 200"
    );
    let mut total = status.len();
    let mut length = None;
    let mut chunked = false;
    let mut close = false;
    loop {
        let raw = line(input, 4096).await?;
        total += raw.len();
        ensure!(total <= 16384, "oversized HTTP headers");
        if raw == b"\r\n" {
            break;
        }
        let raw = std::str::from_utf8(&raw)?;
        let (name, value) = raw.split_once(':').context("invalid HTTP header")?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            ensure!(length.is_none(), "duplicate Content-Length");
            length = Some(value.parse::<usize>()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            ensure!(
                value.eq_ignore_ascii_case("chunked"),
                "unsupported transfer encoding"
            );
            chunked = true;
        } else if name.eq_ignore_ascii_case("connection") {
            close = value.eq_ignore_ascii_case("close");
        } else if name.eq_ignore_ascii_case("content-encoding") {
            ensure!(
                value.eq_ignore_ascii_case("identity"),
                "unexpected response compression"
            );
        }
    }
    ensure!(
        !(chunked && length.is_some()),
        "ambiguous HTTP body framing"
    );
    let mut data = Vec::new();
    if chunked {
        loop {
            let raw = line(input, 128).await?;
            let size = usize::from_str_radix(
                std::str::from_utf8(&raw)?
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or_default(),
                16,
            )?;
            if size == 0 {
                loop {
                    let trailer = line(input, 4096).await?;
                    total += trailer.len();
                    ensure!(total <= 16384, "oversized HTTP trailers");
                    if trailer == b"\r\n" {
                        break;
                    }
                }
                break;
            }
            ensure!(size <= MAX_BODY - data.len(), "oversized HTTP body");
            let start = data.len();
            data.resize(start + size, 0);
            input.read_exact(&mut data[start..]).await?;
            let mut end = [0; 2];
            input.read_exact(&mut end).await?;
            ensure!(end == *b"\r\n", "invalid chunk terminator");
        }
    } else {
        let size = length.context("HTTP body requires explicit framing")?;
        ensure!(size <= MAX_BODY, "oversized HTTP body");
        data.resize(size, 0);
        input.read_exact(&mut data).await?;
    }
    Ok((data, close))
}

fn validate_graphql(data: &[u8], key: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(data)?;
    ensure!(value.get("errors").is_none(), "GraphQL returned errors");
    let rows = value["data"]["rpslObjects"]
        .as_array()
        .context("missing GraphQL object list")?;
    ensure!(
        rows.len() == 1
            && rows[0]["rpslPk"] == key
            && rows[0]["source"] == "PERF"
            && rows[0]["objectClass"] == "aut-num",
        "incorrect GraphQL object identity"
    );
    Ok(())
}

async fn connect(options: &Options) -> Result<BufReader<TcpStream>> {
    let mut socket = TcpStream::connect(options.endpoint).await?;
    socket.set_nodelay(true)?;
    if matches!(
        (options.mode, options.connections),
        (Mode::Whois, Connections::Reused)
    ) {
        socket.write_all(b"!!\n").await?;
    }
    Ok(BufReader::new(socket))
}

#[derive(Default)]
struct Samples {
    times: Vec<u64>,
    bytes: u64,
    errors: BTreeMap<String, u64>,
    successful: u64,
}

async fn worker(options: Arc<Options>, id: usize, warm: Instant, end: Instant) -> Result<Samples> {
    let mut samples = Samples::default();
    let mut connection = None;
    let mut used = 0;
    let mut sequence = id * 997;
    while Instant::now() < end {
        let measured = Instant::now() >= warm;
        let start = Instant::now();
        let key = format!("AS{}", 4_200_000_000_u64 + (sequence % options.keys) as u64);
        sequence = sequence.wrapping_add(104729);
        let outcome = tokio::time::timeout(Duration::from_secs(5),async {
            if used >= 900 || matches!(options.connections,Connections::New) { connection=None; used=0; }
            if connection.is_none() { connection=Some(connect(&options).await?); }
            let socket=connection.as_mut().context("missing connection")?;
            let (bytes,close)=match options.mode {
                Mode::Whois => {
                    socket.get_mut().write_all(format!("!maut-num,{key}\n").as_bytes()).await?;
                    (whois_body(socket,&key).await?,false)
                }
                Mode::Graphql => {
                    let body=json!({"query":format!("{{ rpslObjects(rpslPk:[\"{key}\"],sources:[\"PERF\"],objectClass:[\"aut-num\"]) {{ rpslPk source objectClass }} }}")}).to_string();
                    let keep=if matches!(options.connections,Connections::Reused) {"keep-alive"} else {"close"};
                    socket.get_mut().write_all(format!("POST /graphql/ HTTP/1.1\r\nHost: {}\r\nConnection: {keep}\r\nAccept-Encoding: identity\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",options.endpoint,body.len()).as_bytes()).await?;
                    let (data,close)=http_body(socket).await?;
                    validate_graphql(&data,&key)?;
                    (data.len(),close)
                }
            };
            used+=1;
            if close { connection=None; }
            Ok::<usize,anyhow::Error>(bytes)
        }).await;
        if measured {
            ensure!(
                samples.times.len() < MAX_SAMPLES_PER_CLIENT,
                "benchmark sample budget exhausted"
            );
            samples.times.push(start.elapsed().as_nanos().try_into()?);
        }
        match outcome {
            Ok(Ok(bytes)) if measured => {
                samples.successful += 1;
                samples.bytes += bytes as u64;
            }
            Ok(Ok(_)) => {}
            error => {
                connection = None;
                used = 0;
                let message = match error {
                    Ok(Err(e)) => e.to_string(),
                    _ => "request timed out".into(),
                };
                if measured {
                    *samples.errors.entry(message).or_default() += 1;
                }
            }
        }
    }
    Ok(samples)
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        options.endpoint.ip().is_loopback(),
        "benchmark endpoints must be loopback"
    );
    ensure!(
        (1..=128).contains(&options.clients)
            && (1..=120).contains(&options.seconds)
            && options.warmup <= 60
            && (1..=50000).contains(&options.keys),
        "invalid benchmark limits"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(options.clients.min(8))
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let warm=Instant::now()+Duration::from_secs(options.warmup);
        let end=warm+Duration::from_secs(options.seconds);
        let options=Arc::new(options);
        let mut jobs=Vec::new();
        for id in 0..options.clients { jobs.push(tokio::spawn(worker(options.clone(),id,warm,end))); }
        let mut all=Samples::default();
        for job in jobs {
            let data=job.await??;
            all.times.extend(data.times); all.successful+=data.successful; all.bytes+=data.bytes;
            for (kind,count) in data.errors { *all.errors.entry(kind).or_default()+=count; }
        }
        let seconds=Instant::now().duration_since(warm).as_secs_f64();
        ensure!(!all.times.is_empty(),"no completed benchmark samples");
        all.times.sort_unstable();
        let percentile=|p:f64| all.times[((all.times.len() as f64*p).ceil() as usize).saturating_sub(1)] as f64/1_000_000.0;
        println!("{}",json!({"mode":format!("{:?}",options.mode),"connections":format!("{:?}",options.connections),"clients":options.clients,"keys":options.keys,"warmup_seconds":options.warmup,"requested_seconds":options.seconds,"wall_seconds":seconds,"attempted":all.times.len(),"successful":all.successful,"errors":all.errors,"queries_per_second":all.successful as f64/seconds,"response_body_bytes":all.bytes,"p50_ms":percentile(0.5),"p95_ms":percentile(0.95),"p99_ms":percentile(0.99),"max_ms":percentile(1.0)}));
        ensure!(all.errors.is_empty(),"benchmark had errors; this profile is not valid for a success comparison");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn validates_content_lengths_chunks_and_persistent_boundaries() -> Result<()> {
        let mut response=std::io::Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".as_slice());
        assert_eq!(http_body(&mut response).await?.0, b"{}");
        assert_eq!(http_body(&mut response).await?.0, b"{}");
        for bad in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{}".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}XX",
        ] {
            assert!(http_body(&mut std::io::Cursor::new(bad)).await.is_err());
        }
        Ok(())
    }
    #[tokio::test]
    async fn rejects_incorrect_whois_framing_and_graphql_results() -> Result<()> {
        let body = "aut-num: AS4200000000\nsource: PERF\n";
        let wire = format!("A{}\n{body}C\n", body.len());
        assert_eq!(
            whois_body(&mut std::io::Cursor::new(wire.as_bytes()), "AS4200000000").await?,
            body.len()
        );
        assert!(
            whois_body(&mut std::io::Cursor::new(wire.as_bytes()), "AS4200000001")
                .await
                .is_err()
        );
        assert!(
            whois_body(
                &mut std::io::Cursor::new(wire.replace("C\n", "X\n").as_bytes()),
                "AS4200000000"
            )
            .await
            .is_err()
        );
        assert!(validate_graphql(br#"{"data":{"rpslObjects":[{"rpslPk":"wrong","source":"PERF","objectClass":"aut-num"}]}}"#,"AS4200000000").is_err());
        Ok(())
    }
}
