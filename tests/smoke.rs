use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;

const IO_DEADLINE: Duration = Duration::from_secs(5);

fn cli(config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-irrd"));
    command.arg("--config").arg(config).env("RUST_LOG", "info");
    command
}

fn successful(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "command failed with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

struct Server {
    child: Child,
    log_reader: Option<JoinHandle<()>>,
}

impl Server {
    fn start(config: &Path) -> (Self, SocketAddr) {
        Self::start_listener(config, "WHOIS listening")
    }

    fn start_listener(config: &Path, listener: &'static str) -> (Self, SocketAddr) {
        let child = cli(config)
            .arg("server")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            log_reader: None,
        };
        let stderr = server.child.stderr.take().unwrap();
        let (ready, address) = mpsc::channel();
        server.log_reader = Some(thread::spawn(move || {
            let mut ready = Some(ready);
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let Ok(log) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if log["fields"]["message"] == listener
                    && let Some(sender) = ready.take()
                {
                    let _ = sender.send(line);
                }
            }
        }));

        let log = address
            .recv_timeout(Duration::from_secs(15))
            .expect("server did not report its listening address");
        let event: Value = serde_json::from_str(&log).unwrap();
        let address = event["fields"]["address"]
            .as_str()
            .expect("listening log is missing its address")
            .parse()
            .unwrap();
        (server, address)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(log_reader) = self.log_reader.take() {
            let _ = log_reader.join();
        }
    }
}

fn whois(address: SocketAddr, query: &[u8]) -> String {
    let mut connection = TcpStream::connect_timeout(&address, IO_DEADLINE).unwrap();
    connection.set_read_timeout(Some(IO_DEADLINE)).unwrap();
    connection.set_write_timeout(Some(IO_DEADLINE)).unwrap();
    connection.write_all(query).unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn imports_atomically_and_serves_persisted_objects() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"
database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#,
    )
    .unwrap();
    successful(cli(&config).args(["config", "check"]).output().unwrap());
    assert!(!workspace.path().join("test.redb").exists());

    let snapshot = workspace.path().join("snapshot.gz");
    let mut compressed = GzEncoder::new(fs::File::create(&snapshot).unwrap(), Compression::fast());
    compressed
        .write_all(
            b"aut-num: AS64496\ndescr: Original object\nsource: TEST\n\n\
              mntner: TEST-MNT\nauth: BCRYPT-PW placeholder\n+ continuation\nsource: TEST\n\n\
              route6: 2001:db8::/32\norigin: AS64496\nsource: TEST\n",
        )
        .unwrap();
    compressed.finish().unwrap();
    let stats = successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&stats).unwrap()["objects"],
        3
    );

    let malformed = workspace.path().join("malformed.txt");
    fs::write(
        &malformed,
        b"aut-num: AS64497\ndescr: New object\nsource: TEST\n\nroute: 192.0.2.1/24\nsource: TEST\n",
    )
    .unwrap();
    let result = cli(&config)
        .args(["import", "snapshot", "--source", "TEST", "--file"])
        .arg(&malformed)
        .output()
        .unwrap();
    assert!(!result.status.success());

    let verification = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&verification).unwrap()["objects"],
        3
    );

    let exported = workspace.path().join("export.rpsl");
    let export = || {
        let mut command = cli(&config);
        command
            .args(["export", "snapshot", "--source", "TEST", "--file"])
            .arg(&exported);
        command
    };
    assert!(!export().output().unwrap().status.success());
    assert!(!exported.exists());
    successful(export().arg("--include-sensitive").output().unwrap());
    let exported_bytes = fs::read(&exported).unwrap();
    assert!(String::from_utf8_lossy(&exported_bytes).contains("BCRYPT-PW placeholder"));
    assert!(
        !export()
            .arg("--include-sensitive")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&exported).unwrap(), exported_bytes);

    let restored_config = workspace.path().join("restored.toml");
    fs::write(
        &restored_config,
        fs::read_to_string(&config)
            .unwrap()
            .replace("test.redb", "restored.redb"),
    )
    .unwrap();
    let restored = successful(
        cli(&restored_config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(exported)
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&restored).unwrap()["objects"],
        3
    );

    let (_server, address) = Server::start(&config);
    assert_eq!(
        whois(address, b"as64496\r\n"),
        "aut-num: AS64496\r\ndescr: Original object\r\nsource: TEST\r\n\r\n"
    );
    assert_eq!(whois(address, b"AS64497\r\n"), "% No entries found\r\n");
    let body = "aut-num: AS64496\ndescr: Original object\nsource: TEST\n";
    assert_eq!(
        whois(address, b"!maut-num,AS64496\n"),
        format!("A{}\n{body}C\n", body.len())
    );
    assert_eq!(
        whois(address, b"!maut-num,as64496\n"),
        format!("A{}\n{body}C\n", body.len())
    );
    assert_eq!(whois(address, b"!6as64496\n"), "A14\n2001:db8::/32\nC\n");
    assert_eq!(whois(address, b"!gAS64496\n"), "D\n");
    assert_eq!(whois(address, b"!r2001:db8::/32,o\n"), "A8\nAS64496\nC\n");
    assert_eq!(whois(address, b"!r2001:db8::/33,o\n"), "D\n");
    let route = "route6: 2001:db8::/32\norigin: AS64496\nsource: TEST\n";
    for query in [
        b"!r2001:db8::/32\n".as_slice(),
        b"!r2001:db8:1::/48,l\n",
        b"!r2001:db8::/32,L\n",
        b"!r2001:db8::/31,M\n",
    ] {
        assert_eq!(
            whois(address, query),
            format!("A{}\n{route}C\n", route.len())
        );
    }
    assert_eq!(
        whois(address, b"TEST-MNT\r\n"),
        "mntner: TEST-MNT\r\nsource: TEST\r\n\r\n"
    );
    assert_eq!(
        whois(address, b"-i auth AS64496\r\n"),
        "% Invalid or unsupported query\r\n"
    );
    assert_eq!(
        whois(address, b"-i origin as64496\r\n"),
        format!("{}\n\n\n", route.trim_end())
    );
    assert_eq!(
        whois(address, b"-x 2001:db8::/32\r\n"),
        format!("{}\n\n\n", route.trim_end())
    );
}

#[test]
fn whois_pipelines_commands_and_keeps_source_selection_per_connection() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "test.redb"
sources = ["TEST", "OTHER"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#,
    )
    .unwrap();
    for source in ["TEST", "OTHER"] {
        let snapshot = workspace.path().join(format!("{source}.rpsl"));
        let mut records = format!("aut-num: AS64496\nsource: {source}\n");
        if source == "TEST" {
            records.push_str(
                "\nas-set: AS-ROOT\nmembers: AS64496, AS-CHILD\nsource: TEST\n\n\
                 as-set: AS-CHILD\nmembers: AS64497\nsource: TEST\n\n\
                 route: 192.0.2.0/24\norigin: AS64496\nmnt-by: TEST-MNT\nsource: TEST\n",
            );
        }
        fs::write(&snapshot, records).unwrap();
        successful(
            cli(&config)
                .args(["import", "snapshot", "--source", source, "--file"])
                .arg(snapshot)
                .output()
                .unwrap(),
        );
    }

    let (_server, address) = Server::start(&config);
    let other = "aut-num: AS64496\nsource: OTHER\n";
    let test = "aut-num: AS64496\nsource: TEST\n";
    assert_eq!(
        whois(
            address,
            b"!!\n!s-lc\n!sother,test\n!sMISSING\n!s-lc\n!maut-num,AS64496\n!Q\n"
        ),
        format!(
            "A11\nTEST,OTHER\nC\nC\nF Invalid source selection\nA11\nOTHER,TEST\nC\nA{}\n{other}C\n",
            other.len()
        )
    );
    assert_eq!(
        whois(address, b"!maut-num,AS64496\n"),
        format!("A{}\n{test}C\n", test.len())
    );
    for (query, body) in [
        ("!iAS-ROOT", "AS-CHILD AS64496"),
        ("!iAS-ROOT,1", "AS64496 AS64497"),
        ("!a4AS-ROOT", "192.0.2.0/24"),
        (
            "!otest-mnt",
            "route: 192.0.2.0/24\norigin: AS64496\nmnt-by: TEST-MNT\nsource: TEST",
        ),
    ] {
        assert_eq!(
            whois(address, format!("{query}\n").as_bytes()),
            format!("A{}\n{body}\nC\n", body.len() + 1)
        );
    }
    drop(_server);
    let purge = || {
        let mut command = cli(&config);
        command.args(["source", "purge", "--source", "OTHER"]);
        command
    };
    assert!(!purge().output().unwrap().status.success());
    for _ in 0..2 {
        successful(purge().arg("--yes").output().unwrap());
    }
    let verified = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&verified).unwrap()["objects"],
        4
    );
    let (_server, address) = Server::start(&config);
    assert_eq!(
        whois(address, b"!!\n!sOTHER\n!maut-num,AS64496\n!q\n"),
        "C\nD\n"
    );
    assert_eq!(
        whois(address, b"!maut-num,AS64496\n"),
        format!("A{}\n{test}C\n", test.len())
    );
}

#[test]
fn graphql_serves_filtered_registry_objects_over_http() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
http_listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#,
    )
    .unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(
        &snapshot,
        "mntner: TEST-MNT\nauth: BCRYPT-PW synthetic-secret\nsource: TEST\n",
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(snapshot)
            .output()
            .unwrap(),
    );
    let (_server, address) = Server::start_listener(&config, "GraphQL listening");
    let body = serde_json::json!({
        "query": "{ rpslObjects(rpslPk: [\"TEST-MNT\"]) { rpslPk objectClass source objectText } }"
    });
    let body = body.to_string();
    let request = format!(
        "POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let response = whois(address, request.as_bytes());
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(!response.contains("synthetic-secret"));
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    let body: Value = serde_json::from_str(body).unwrap();
    assert!(body.get("errors").is_none(), "{body}");
    let object = &body["data"]["rpslObjects"][0];
    assert_eq!(object["rpslPk"], "TEST-MNT");
    assert_eq!(object["objectClass"], "mntner");
    assert_eq!(object["source"], "TEST");
    assert!(
        object["objectText"]
            .as_str()
            .unwrap()
            .contains("source: TEST")
    );
}

#[test]
fn nrtm3_commits_objects_and_serial_together_over_file_and_tcp() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 3
max_snapshot_bytes = 4096
"#,
    )
    .unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(&snapshot, "aut-num: AS64496\nsource: TEST\n").unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(snapshot)
            .output()
            .unwrap(),
    );

    let transcript = workspace.path().join("update.txt");
    fs::write(
        &transcript,
        "%START Version: 3 TEST 42-42\n\nADD 42\n\naut-num: AS64497\nsource: TEST\n\n%END OTHER\n",
    )
    .unwrap();
    let apply = || {
        let mut command = cli(&config);
        command
            .args([
                "source",
                "apply-nrtm3",
                "--source",
                "TEST",
                "--base-serial",
                "41",
                "--file",
            ])
            .arg(&transcript);
        command
    };
    let status = || {
        let output = successful(
            cli(&config)
                .args(["source", "status", "--source", "TEST"])
                .output()
                .unwrap(),
        );
        serde_json::from_slice::<Value>(&output).unwrap()["serial"].clone()
    };
    assert!(!apply().output().unwrap().status.success());
    assert_eq!(status(), Value::Null);

    fs::write(
        &transcript,
        "%START Version: 3 TEST 42-42\n\nADD 42\n\naut-num: AS64497\nsource: TEST\n\n%END TEST\n",
    )
    .unwrap();
    successful(apply().output().unwrap());
    assert_eq!(status(), 42);
    assert!(!apply().output().unwrap().status.success());

    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = upstream.local_addr().unwrap().to_string();
    let fetch = thread::spawn(move || {
        let (mut socket, _) = upstream.accept().unwrap();
        socket.set_read_timeout(Some(IO_DEADLINE)).unwrap();
        let mut query = String::new();
        BufReader::new(&mut socket).read_line(&mut query).unwrap();
        assert_eq!(query, "-g TEST:3:43-LAST\n");
        socket
            .write_all(b"%START Version: 3 TEST 43-43\n\nDEL 43\n\naut-num: AS64496\n\n%END TEST\n")
            .unwrap();
    });
    let sync = cli(&config)
        .args([
            "source",
            "sync-nrtm3",
            "--source",
            "TEST",
            "--endpoint",
            &endpoint,
        ])
        .output()
        .unwrap();
    successful(sync);
    fetch.join().unwrap();
    assert_eq!(status(), 43);

    let (_server, address) = Server::start(&config);
    assert_eq!(whois(address, b"!maut-num,AS64496\n"), "D\n");
    let remaining = "aut-num: AS64497\nsource: TEST\n";
    assert_eq!(
        whois(address, b"!maut-num,AS64497\n"),
        format!("A{}\n{remaining}C\n", remaining.len())
    );
}
