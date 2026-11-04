use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;

const IO_DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn authoritative_nrtm3_publishes_changes_and_rejects_ranges_across_rebases() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "registry.redb"
sources = ["TEST"]
authoritative_sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096

[nrtm3_serve]
TEST = ["127.0.0.1"]
"#,
    )
    .unwrap();
    let snapshot = workspace.path().join("bootstrap.rpsl");
    let hash = bcrypt::hash_with_salt("journal test secret", 4, [9; 16]).unwrap();
    fs::write(
        &snapshot,
        format!("mntner: TEST-MNT\nauth: BCRYPT-PW {hash}\nmnt-by: TEST-MNT\nsource: TEST\n"),
    )
    .unwrap();
    let restore = || {
        let mut command = cli(&config);
        command
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(&snapshot);
        command
    };
    successful(restore().output().unwrap());
    let exported = workspace.path().join("public.rpsl");
    let report = successful(
        cli(&config)
            .args([
                "export", "snapshot", "--source", "TEST", "--public", "--file",
            ])
            .arg(&exported)
            .output()
            .unwrap(),
    );
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report["serial"], 0);
    assert_eq!(report["objects"], 1);
    assert!(!fs::read_to_string(&exported).unwrap().contains("BCRYPT-PW"));

    let submission = workspace.path().join("submission.rpsl");
    let change = |body: &str, delete: bool| {
        fs::write(&submission, body).unwrap();
        let mut command = cli(&config);
        command
            .args(["update", "--source", "TEST", "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if delete {
            command.arg("--delete");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"journal test secret\n")
            .unwrap();
        successful(child.wait_with_output().unwrap());
    };
    change(
        "person: Example Operator\naddress: Example Road\nphone: +1 202 555 0100\ne-mail: noc@example.net\nnic-hdl: OPER-TEST\nmnt-by: TEST-MNT\nsource: TEST\n",
        false,
    );
    change(
        "person: Delete Request\nnic-hdl: OPER-TEST\nsource: TEST\n",
        true,
    );
    let (server, address) = Server::start(&config);
    let transcript = whois(address, b"-g TEST:3:1-LAST\n");
    assert!(transcript.starts_with("%START Version: 3 TEST 1-2\n"));
    assert!(transcript.contains("DEL 2\n\nperson: Example Operator\n"));
    assert!(!transcript.contains("Delete Request"));
    drop(server);

    successful(restore().args(["--serial-floor", "50"]).output().unwrap());
    let status = successful(
        cli(&config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status).unwrap()["local_serial"],
        51
    );
    let (_server, address) = Server::start(&config);
    assert!(whois(address, b"-g TEST:3:3-LAST\n").contains("range is unavailable"));
    assert!(whois(address, b"-g TEST:3:52-LAST\n").contains("no newer updates"));
}

#[test]
fn authoritative_cli_checks_maintainers_and_rolls_back_a_mixed_batch() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "test.redb"
sources = ["TEST", "MIRROR"]
authoritative_sources = ["TEST"]
listen = "127.0.0.1:0"
http_listen = "127.0.0.1:0"
http_authoritative_updates = true
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096

[authoritative_scope]
prefixes = ["198.51.100.128/25"]
"#,
    )
    .unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    let root_hash = bcrypt::hash_with_salt("root test secret", 4, [7; 16]).unwrap();
    let new_hash = bcrypt::hash_with_salt("new test secret", 4, [8; 16]).unwrap();
    let person = |name: &str, key: &str, mnt: &str| {
        format!(
            "person: {name}\naddress: Example Road\nphone: +1 202 555 0100\ne-mail: noc@example.net\nnic-hdl: {key}\nmnt-by: {mnt}\nsource: TEST\n"
        )
    };
    let mntner = |key: &str, hash: &str, mnt: &str| {
        format!(
            "mntner: {key}\nadmin-c: ADMIN-TEST\nupd-to: noc@example.net\nauth: BCRYPT-PW {hash}\nmnt-by: {mnt}\nsource: TEST\n"
        )
    };
    fs::write(
        &snapshot,
        format!(
            "{}\n{}\n{}\ninetnum: 192.0.2.0 - 192.0.2.255\nmnt-by: ROOT-MNT\nsource: TEST\n\naut-num: AS64496\nmnt-by: ROOT-MNT\nsource: TEST\n\nas-block: AS64497 - AS64505\nmnt-by: NEW-MNT\nsource: TEST\n\nas-block: AS64497 - AS64499\nmnt-by: ROOT-MNT\nsource: TEST\n",
            mntner("ROOT-MNT", &root_hash.to_string(), "ROOT-MNT"),
            person("Test Admin", "ADMIN-TEST", "ROOT-MNT"),
            mntner("NEW-MNT", &new_hash.to_string(), "NEW-MNT")
        ),
    )
    .unwrap();
    let import = || {
        let mut command = cli(&config);
        command
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&snapshot);
        command
    };
    assert!(!import().output().unwrap().status.success());
    successful(import().arg("--authoritative-restore").output().unwrap());
    let submission = workspace.path().join("submission.rpsl");
    let submit = |source: &str, body: &str, secrets: &str, delete: bool| {
        fs::write(&submission, body).unwrap();
        let mut command = cli(&config);
        command
            .args(["update", "--source", source, "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if delete {
            command.arg("--delete");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(secrets.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let contact = person("Test Operator", "OPER-TEST", "ROOT-MNT");
    assert!(
        !submit("MIRROR", &contact, "root test secret\n", false)
            .status
            .success()
    );
    let failed = submit("TEST", &contact, "bad test secret\n", false);
    assert!(!failed.status.success());
    assert!(!String::from_utf8_lossy(&failed.stderr).contains("bad test secret"));
    let mixed = format!(
        "{contact}\n{}",
        person("Missing Owner", "MISS-TEST", "MISSING-MNT")
    );
    assert!(
        !submit("TEST", &mixed, "root test secret\n", false)
            .status
            .success()
    );
    let verification = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&verification).unwrap()["objects"],
        7
    );
    successful(submit("TEST", &contact, "root test secret\n", false));
    let unprovisioned = mntner("UNPROVISIONED-MNT", &new_hash.to_string(), "ROOT-MNT");
    assert!(
        !submit(
            "TEST",
            &unprovisioned,
            "root test secret\nnew test secret\n",
            false
        )
        .status
        .success()
    );
    let new = mntner("NEW-MNT", &new_hash.to_string(), "ROOT-MNT, NEW-MNT");
    successful(submit(
        "TEST",
        &new,
        "root test secret\nnew test secret\n",
        false,
    ));
    let transferred = person("Test Operator", "OPER-TEST", "NEW-MNT");
    for single in ["root test secret\n", "new test secret\n"] {
        assert!(!submit("TEST", &transferred, single, false).status.success());
    }
    successful(submit(
        "TEST",
        &transferred,
        "root test secret\nnew test secret\n",
        false,
    ));
    let admin = "person: Test Admin\nnic-hdl: ADMIN-TEST\nsource: TEST\n";
    assert!(
        !submit("TEST", admin, "root test secret\n", true)
            .status
            .success()
    );
    let operator = "person: Test Operator\nnic-hdl: OPER-TEST\nsource: TEST\n";
    successful(submit("TEST", operator, "new test secret\n", true));
    let history = successful(
        cli(&config)
            .args(["source", "history", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    let entries: Vec<Value> = serde_json::from_slice(&history).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["action"], "delete");
    assert_eq!(entries[0]["key"], "OPER-TEST");
    assert!(!String::from_utf8_lossy(&history).contains("test secret"));
    let set = "as-set: AS64496:AS-EXAMPLE\nmembers: AS64496\nmnt-by: NEW-MNT\nsource: TEST\n";
    let rejected = submit("TEST", set, "new test secret\n", false);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("parent maintainer authentication"));
    successful(submit(
        "TEST",
        set,
        "root test secret\nnew test secret\n",
        false,
    ));
    successful(submit(
        "TEST",
        &set.replace("members: AS64496", "members: AS64497"),
        "new test secret\n",
        false,
    ));
    assert!(
        !submit(
            "TEST",
            &set.replace("AS64496:", ""),
            "new test secret\n",
            false
        )
        .status
        .success()
    );
    let route_set = "route-set: AS64497:RS-EXAMPLE\nmp-members: 2001:db8::/32^+\nmnt-by: NEW-MNT\nsource: TEST\n";
    successful(submit("TEST", route_set, "new test secret\n", false));
    let aut_num = "aut-num: AS64497\nas-name: AS-EXAMPLE\nadmin-c: ADMIN-TEST\ntech-c: ADMIN-TEST\nmnt-by: NEW-MNT\nsource: TEST\n";
    let wrong_block = submit("TEST", aut_num, "new test secret\n", false);
    assert!(!wrong_block.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_block.stderr).contains("parent maintainer authentication")
    );
    let unallocated = submit(
        "TEST",
        &aut_num.replace("AS64497", "AS64600"),
        "new test secret\n",
        false,
    );
    assert!(!unallocated.status.success());
    assert!(String::from_utf8_lossy(&unallocated.stderr).contains("no covering AS-block"));
    successful(submit(
        "TEST",
        aut_num,
        "root test secret\nnew test secret\n",
        false,
    ));
    let updated_aut_num = aut_num
        .replace("AS64497", "AS64496")
        .replace("NEW-MNT", "ROOT-MNT");
    assert!(
        !submit("TEST", &updated_aut_num, "new test secret\n", false)
            .status
            .success()
    );
    successful(submit(
        "TEST",
        &updated_aut_num,
        "root test secret\n",
        false,
    ));
    let (_server, address) = Server::start(&config);
    assert!(whois(address, b"!mas-set,AS64496:AS-EXAMPLE\n").contains("members: AS64497"));
    assert_eq!(whois(address, b"!mperson,OPER-TEST\n"), "D\n");
    assert!(!whois(address, b"!mmntner,NEW-MNT\n").contains("BCRYPT-PW"));
    drop(_server);
    let (_server, http) = Server::start_listener(&config, "GraphQL listening");
    let request = |method: &str, path: &str, body: Value| {
        let body = body.to_string();
        let raw = whois(http, format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ).as_bytes());
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
        assert!(!raw.contains("root test secret"));
        serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    let contact = person("HTTP Operator", "HTTP-TEST", "ROOT-MNT");
    let payload =
        serde_json::json!({"objects":[{"object_text":contact}],"passwords":["root test secret"]});
    let accepted = request("POST", "/v1/submit/", payload.clone());
    assert_eq!(accepted["summary"]["successful_create"], 1);
    assert!(accepted["objects"][0]["new_object_text"].is_null());
    let visible = request(
        "POST",
        "/graphql/",
        serde_json::json!({
            "query":"{ rpslObjects(rpslPk: [\"HTTP-TEST\"]) { rpslPk } }"
        }),
    );
    assert_eq!(visible["data"]["rpslObjects"][0]["rpslPk"], "HTTP-TEST");
    assert_eq!(
        request("DELETE", "/v1/submit/", payload.clone())["summary"]["successful_delete"],
        1
    );
    assert_eq!(
        request("POST", "/v1/submit/", payload)["summary"]["failed"],
        1
    );
    let route = "route: 192.0.2.128/25\norigin: AS64496\nmnt-by: NEW-MNT\nsource: TEST\n";
    let route_payload = |passwords: &[&str], body: &str| serde_json::json!({"objects":[{"object_text":body}],"passwords":passwords});
    assert_eq!(
        request(
            "POST",
            "/v1/submit/",
            route_payload(&["new test secret"], route)
        )["summary"]["failed"],
        1
    );
    let outside_scope = route.replace("192.0.2.128/25", "198.51.100.0/24");
    assert_eq!(
        request(
            "POST",
            "/v1/submit/",
            route_payload(&["new test secret"], &outside_scope)
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            "POST",
            "/v1/submit/",
            route_payload(
                &["root test secret", "new test secret"],
                &route.replace("192.0.2.128", "192.00.2.128")
            ),
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            "POST",
            "/v1/submit/",
            route_payload(&["root test secret", "new test secret"], route)
        )["summary"]["successful_create"],
        1
    );
    assert_eq!(
        request(
            "DELETE",
            "/v1/submit/",
            route_payload(&["new test secret"], route)
        )["summary"]["successful_delete"],
        1
    );
}

#[test]
fn legacy_md5_auth_is_source_opt_in_and_can_rotate_to_bcrypt() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    let settings = r#"database = "test.redb"
sources = ["TEST"]
authoritative_sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#;
    fs::write(&config, settings).unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    let maintainer = |auth: &str| {
        format!(
            "mntner: LEGACY-MNT\nadmin-c: ADMIN-TEST\nupd-to: noc@example.net\nauth: {auth}\nmnt-by: LEGACY-MNT\nsource: TEST\n"
        )
    };
    let legacy = maintainer("MD5-PW $1$salt$qJH7.N4xYta3aEG/dfqo/0");
    fs::write(
        &snapshot,
        format!(
            "{legacy}\nperson: Admin Test\nnic-hdl: ADMIN-TEST\nmnt-by: LEGACY-MNT\nsource: TEST\n"
        ),
    )
    .unwrap();
    successful(
        cli(&config)
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(snapshot)
            .output()
            .unwrap(),
    );
    let submission = workspace.path().join("update.rpsl");
    let modern = maintainer(&format!(
        "BCRYPT-PW {}",
        bcrypt::hash_with_salt("replacement test secret", 4, [9; 16]).unwrap()
    ));
    let submit = |body: &str, passwords: &[u8]| {
        fs::write(&submission, body).unwrap();
        let mut child = cli(&config)
            .args(["update", "--source", "TEST", "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(passwords).unwrap();
        child.wait_with_output().unwrap()
    };
    let both = b"password\nreplacement test secret\n";
    assert!(!submit(&modern, both).status.success());
    fs::write(
        &config,
        format!("{settings}legacy_md5_sources = [\"TEST\"]\n"),
    )
    .unwrap();
    successful(submit(&modern, both));
    assert!(!submit(&legacy, both).status.success());
    let history = successful(
        cli(&config)
            .args(["source", "history", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    let history: Vec<Value> = serde_json::from_slice(&history).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["key"], "LEGACY-MNT");
}

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
max_connections_per_ip = 1
connection_limit_exempt_ips = ["::ffff:127.0.0.1"]
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
    let mut held = TcpStream::connect_timeout(&address, IO_DEADLINE).unwrap();
    held.set_read_timeout(Some(IO_DEADLINE)).unwrap();
    held.write_all(b"!!\n!s-lc\n").unwrap();
    let mut ready = [0; b"A11\nTEST,OTHER\nC\n".len()];
    held.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"A11\nTEST,OTHER\nC\n");
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
    held.write_all(b"!q\n").unwrap();
    drop(held);
    assert_eq!(
        whois(address, b"!maut-num,AS64496\n"),
        format!("A{}\n{test}C\n", test.len())
    );
    assert_eq!(
        whois(
            address,
            b"-k -s OTHER\n!s-lc\n!maut-num,AS64496\n-a\n!s-lc\n!q\n"
        ),
        format!(
            "%  No entries found for the selected source(s).\n\n\nA6\nOTHER\nC\nA{}\n{other}C\n%  No entries found for the selected source(s).\n\n\nA11\nTEST,OTHER\nC\n",
            other.len()
        )
    );
    assert_eq!(
        whois(address, b"-V fixture !maut-num,AS64496\n"),
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
fn nrtm3_cli_decodes_only_the_configured_peer_and_rolls_back_bad_utf8() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("irrd.toml");
    let basic = "database = \"registry.redb\"
sources = [\"TEST\"]
listen = \"127.0.0.1:0\"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 8192
";
    fs::write(&config, basic).unwrap();
    let snapshot = work.path().join("base.rpsl");
    fs::write(
        &snapshot,
        "aut-num: AS64496
source: TEST
",
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    for encoding in [None, Some("iso-8859-1")] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        fs::write(
            &config,
            format!(
                "{basic}
[nrtm3_sync.TEST]
endpoint = {endpoint:?}
interval_seconds = 60
object_class_filter = [\"aut-num\"]
{}",
                encoding
                    .map(|value| format!(
                        "encoding = {value:?}
"
                    ))
                    .unwrap_or_default()
            ),
        )
        .unwrap();
        let peer = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(IO_DEADLINE)).unwrap();
            let mut query = String::new();
            BufReader::new(&mut socket).read_line(&mut query).unwrap();
            assert_eq!(
                query,
                "-g TEST:3:10-LAST
"
            );
            socket
                .write_all(
                    b"%START Version: 3 TEST 10-11

ADD 10

aut-num: AS64497
source: TEST

ADD 11

aut-num: AS64498
descr: Andr\xe9
source: TEST

%END TEST
",
                )
                .unwrap();
        });
        let result = cli(&config)
            .args([
                "source",
                "sync-nrtm3",
                "--source",
                "TEST",
                "--endpoint",
                &endpoint,
                "--base-serial",
                "9",
            ])
            .output()
            .unwrap();
        assert_eq!(result.status.success(), encoding.is_some());
        peer.join().unwrap();
        let status: Value = serde_json::from_slice(&successful(
            cli(&config)
                .args(["source", "status", "--source", "TEST"])
                .output()
                .unwrap(),
        ))
        .unwrap();
        let count: Value = serde_json::from_slice(&successful(
            cli(&config).args(["database", "count"]).output().unwrap(),
        ))
        .unwrap();
        if encoding.is_some() {
            assert_eq!(status["serial"], 11);
            assert_eq!(count["objects"], 3);
            let export = work.path().join("public.rpsl");
            successful(
                cli(&config)
                    .args([
                        "export", "snapshot", "--source", "TEST", "--public", "--file",
                    ])
                    .arg(&export)
                    .output()
                    .unwrap(),
            );
            assert!(fs::read_to_string(export).unwrap().contains("André"));
        } else {
            assert!(status["serial"].is_null());
            assert_eq!(count["objects"], 1);
            assert!(String::from_utf8_lossy(&result.stderr).contains("UTF-8"));
        }
    }
}

#[test]
fn bounded_nrtm3_cli_rejects_a_peer_that_exceeds_the_requested_range() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nmax_connections = 4\nio_timeout_seconds = 2\nmax_snapshot_bytes = 8192\n").unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(&snapshot, "aut-num: AS64496\nsource: TEST\n").unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(snapshot)
            .output()
            .unwrap(),
    );
    for (request, response, initial, succeeds) in [
        (
            "-g TEST:3:10-10\n",
            "%START Version: 3 TEST 10-10\n\nADD 10\n\naut-num: AS64497\nsource: TEST\n\n%END TEST\n",
            true,
            true,
        ),
        (
            "-g TEST:3:11-11\n",
            "%START Version: 3 TEST 11-12\n\nADD 11\n\naut-num: AS64498\nsource: TEST\n\nADD 12\n\naut-num: AS64499\nsource: TEST\n\n%END TEST\n",
            false,
            false,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let peer = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(IO_DEADLINE)).unwrap();
            let mut query = String::new();
            BufReader::new(&mut socket).read_line(&mut query).unwrap();
            assert_eq!(query, request);
            socket.write_all(response.as_bytes()).unwrap();
        });
        let mut command = cli(&config);
        command.args([
            "source",
            "sync-nrtm3",
            "--source",
            "TEST",
            "--endpoint",
            &endpoint,
            "--end-serial",
            if initial { "10" } else { "11" },
        ]);
        if initial {
            command.args(["--base-serial", "9"]);
        }
        let result = command.output().unwrap();
        assert_eq!(result.status.success(), succeeds);
        if !succeeds {
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("exceeds requested end serial")
            );
        }
        peer.join().unwrap();
        let status = successful(
            cli(&config)
                .args(["source", "status", "--source", "TEST"])
                .output()
                .unwrap(),
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&status).unwrap()["serial"],
            10
        );
        let count = successful(cli(&config).args(["database", "count"]).output().unwrap());
        assert_eq!(
            serde_json::from_slice::<Value>(&count).unwrap()["objects"],
            2
        );
    }
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

[nrtm3_serve]
TEST = ["127.0.0.1"]
"#,
    )
    .unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(&snapshot, "aut-num: AS64496\nsource: TEST\n").unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&snapshot)
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

    fs::write(
        &transcript,
        "%START Version: 3 TEST 44-44\n\nADD 44\n\nmntner: TEST-MNT\nauth: BCRYPT-PW synthetic-secret\nsource: TEST\n\n%END TEST\n",
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["source", "apply-nrtm3", "--source", "TEST", "--file"])
            .arg(&transcript)
            .output()
            .unwrap(),
    );
    assert!(
        !cli(&config)
            .args(["database", "compact"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let compacted = successful(
        cli(&config)
            .args(["database", "compact", "--yes"])
            .output()
            .unwrap(),
    );
    assert!(serde_json::from_slice::<Value>(&compacted).unwrap()["file_bytes_after"].is_u64());
    assert_eq!(status(), 44);

    let (_server, address) = Server::start(&config);
    let journal = whois(address, b"-g test:3:42-LAST\n");
    assert!(journal.starts_with("%START Version: 3 TEST 42-44\n"));
    assert!(journal.contains("ADD 42\n"));
    assert!(journal.contains("DEL 43\n"));
    assert!(journal.contains("ADD 44\n"));
    assert!(journal.ends_with("%END TEST\n\n\n"));
    assert!(!journal.contains("synthetic-secret"));
    assert_eq!(
        whois(address, b"-g OTHER:3:42-LAST\n"),
        "%% ERROR: Access denied\n\n\n"
    );
    assert_eq!(whois(address, b"!maut-num,AS64496\n"), "D\n");
    let remaining = "aut-num: AS64497\nsource: TEST\n";
    assert_eq!(
        whois(address, b"!maut-num,AS64497\n"),
        format!("A{}\n{remaining}C\n", remaining.len())
    );

    let replica = workspace.path().join("replica.toml");
    fs::write(
        &replica,
        fs::read_to_string(&config)
            .unwrap()
            .replace("test.redb", "replica.redb"),
    )
    .unwrap();
    successful(
        cli(&replica)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    let endpoint = address.to_string();
    let sync = || {
        let mut command = cli(&replica);
        command.args([
            "source",
            "sync-nrtm3",
            "--source",
            "TEST",
            "--endpoint",
            &endpoint,
        ]);
        command
    };
    let replicated = successful(sync().args(["--base-serial", "41"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&replicated).unwrap()["serial"],
        44
    );
    let unchanged = successful(sync().output().unwrap());
    let unchanged: Value = serde_json::from_slice(&unchanged).unwrap();
    assert_eq!(unchanged["serial"], 44);
    assert_eq!(unchanged["up_to_date"], true);
}

#[test]
fn scheduled_mirror_retries_a_bad_transcript_and_publishes_a_valid_update() {
    let workspace = tempfile::tempdir().unwrap();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    upstream.set_nonblocking(true).unwrap();
    let endpoint = upstream.local_addr().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        format!(
            r#"database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096

[nrtm3_sync.TEST]
endpoint = "{endpoint}"
interval_seconds = 1
base_serial = 41
"#
        ),
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
    let fixture = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        for trailer in ["OTHER", "TEST"] {
            let mut stream = loop {
                match upstream.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "scheduled peer was not contacted"
                        );
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("accepting scheduled peer: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(IO_DEADLINE)).unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            assert_eq!(request, "-g TEST:3:42-LAST\n");
            write!(stream, "%START Version: 3 TEST 42-42\n\nADD 42\n\naut-num: AS64497\nsource: TEST\n\n%END {trailer}\n").unwrap();
        }
    });
    let (server, address) = Server::start(&config);
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let response = whois(address, b"!maut-num,AS64497\n");
        if response.contains("aut-num: AS64497") {
            break;
        }
        assert_eq!(response, "D\n");
        assert!(
            Instant::now() < deadline,
            "scheduled update was not published"
        );
        thread::sleep(Duration::from_millis(50));
    }
    fixture.join().unwrap();
    drop(server);
    let status = successful(
        cli(&config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status).unwrap()["serial"],
        42
    );
}

#[test]
fn removed_validator_cli_and_configuration_fail_without_creating_a_database() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    let base = r#"database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#;
    fs::write(&config, base).unwrap();
    let output = cli(&config).args(["rpki", "validate"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    for removed in [
        r#"authoritative_roa_file = "roas.json""#,
        r#"authoritative_roa_exempt_sources = ["TEST"]"#,
        "[rpki]\ncache_dir = \"/tmp/unused\"\n",
    ] {
        fs::write(&config, format!("{base}{removed}")).unwrap();
        let output = cli(&config).args(["config", "check"]).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));
    }
    assert!(!workspace.path().join("test.redb").exists());
}

#[test]
fn nrtm4_cli_verifies_signed_metadata_and_the_same_compressed_file_it_parses() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::{
        ecdsa::{Signature, SigningKey, signature::Signer},
        pkcs8::{EncodePublicKey, LineEnding},
    };
    use sha2::{Digest, Sha256};

    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        r#"database = "test.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 4096
"#,
    )
    .unwrap();
    let session = "123e4567-e89b-42d3-a456-426614174000";
    let header = serde_json::json!({
        "nrtm_version": 4, "type": "snapshot", "source": "TEST", "session_id": session, "version": 7
    });
    let object = serde_json::json!({ "object": "aut-num: AS64496\nsource: TEST\n" });
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    writeln!(gzip, "\x1e{header}\n\x1e{object}").unwrap();
    let mut compressed = gzip.finish().unwrap();
    let hash: String = Sha256::digest(&compressed)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let file = workspace.path().join("snapshot.gz");
    fs::write(&file, &compressed).unwrap();

    let now = time::OffsetDateTime::now_utc();
    let timestamp = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let payload = serde_json::json!({
        "nrtm_version": 4, "type": "notification", "source": "TEST", "session_id": session,
        "version": 7, "timestamp": timestamp, "deltas": [],
        "snapshot": { "version": 7, "url": "snapshot.json.gz", "hash": hash }
    });
    let signing_key = SigningKey::from_slice(&[7; 32]).unwrap();
    let notification = workspace.path().join("update-notification-file.jose");
    let sign = |payload: &Value| {
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap())
        );
        let signature: Signature = signing_key.sign(signing_input.as_bytes());
        fs::write(
            &notification,
            format!(
                "{signing_input}.{}",
                URL_SAFE_NO_PAD.encode(signature.to_bytes())
            ),
        )
        .unwrap();
    };
    sign(&payload);
    let public_key = workspace.path().join("public.pem");
    fs::write(
        &public_key,
        signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    )
    .unwrap();

    let verify = || {
        let mut command = cli(&config);
        command
            .args([
                "source",
                "verify-nrtm4",
                "--source",
                "TEST",
                "--notification",
            ])
            .arg(&notification)
            .arg("--public-key")
            .arg(&public_key)
            .arg("--snapshot")
            .arg(&file);
        command
    };
    let result = successful(verify().output().unwrap());
    let result: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result["file"]["version"], 7);
    assert_eq!(result["file"]["objects"], 1);
    compressed.push(0);
    fs::write(&file, &compressed).unwrap();
    let tampered = verify().output().unwrap();
    assert!(!tampered.status.success());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("hash does not match"));
    assert!(!workspace.path().join("test.redb").exists());
    compressed.pop();
    fs::write(&file, &compressed).unwrap();
    let apply = || {
        let mut command = cli(&config);
        command
            .args([
                "source",
                "apply-nrtm4",
                "--source",
                "TEST",
                "--notification",
            ])
            .arg(&notification);
        command
    };
    let initial = successful(
        apply()
            .arg("--public-key")
            .arg(&public_key)
            .arg("--file")
            .arg(&file)
            .output()
            .unwrap(),
    );
    let initial: Value = serde_json::from_slice(&initial).unwrap();
    assert_eq!(initial["version"], 7);
    assert_eq!(initial["replaced_snapshot"], true);

    let delta = workspace.path().join("delta.json");
    let delta_header = serde_json::json!({
        "nrtm_version":4,"type":"delta","source":"TEST","session_id":session,"version":8,
    });
    let add =
        serde_json::json!({"action":"add_modify","object":"aut-num: AS64497\nsource: TEST\n"});
    let delete =
        serde_json::json!({"action":"delete","object_class":"aut-num","primary_key":"AS64496"});
    let body = format!("\x1e{delta_header}\n\x1e{add}\n\x1e{delete}\n");
    let write_delta = |contents: &str| {
        fs::write(&delta, contents).unwrap();
        let hash: String = Sha256::digest(contents.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut updated = payload.clone();
        updated["version"] = 8.into();
        updated["deltas"] = serde_json::json!([{"version":8,"url":"delta.json","hash":hash}]);
        sign(&updated);
    };
    write_delta(&format!("{body}\x1e{{\n"));
    assert!(
        !apply()
            .arg("--file")
            .arg(&delta)
            .output()
            .unwrap()
            .status
            .success()
    );
    let status = successful(
        cli(&config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status).unwrap()["nrtm4"]["version"],
        7
    );
    let verified = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&verified).unwrap()["objects"],
        1
    );
    write_delta(&body);
    let applied = successful(apply().arg("--file").arg(&delta).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&applied).unwrap()["version"],
        8
    );
    let replay = successful(apply().output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&replay).unwrap()["records"],
        0
    );
    sign(&payload);
    assert!(
        !apply()
            .arg("--file")
            .arg(&file)
            .output()
            .unwrap()
            .status
            .success()
    );
    let backup = workspace.path().join("registry.backup");
    let backup_command = || {
        let mut command = cli(&config);
        command.args(["database", "backup", "--file"]).arg(&backup);
        command
    };
    successful(backup_command().output().unwrap());
    assert!(!backup_command().output().unwrap().status.success());
    let restore_config = workspace.path().join("restored.toml");
    let original_config = fs::read_to_string(&config).unwrap();
    fs::write(
        &restore_config,
        original_config.replace("test.redb", "restored.redb"),
    )
    .unwrap();
    let restore = |input: &Path| {
        cli(&restore_config)
            .args(["database", "restore", "--file"])
            .arg(input)
            .output()
            .unwrap()
    };
    let corrupted = workspace.path().join("corrupted.backup");
    let mut corrupt_bytes = fs::read(&backup).unwrap();
    *corrupt_bytes.last_mut().unwrap() ^= 1;
    fs::write(&corrupted, corrupt_bytes).unwrap();
    assert!(!restore(&corrupted).status.success());
    assert!(!workspace.path().join("restored.redb").exists());
    successful(restore(&backup));
    assert!(!restore(&backup).status.success());
    let restored_status = successful(
        cli(&restore_config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&restored_status).unwrap()["nrtm4"]["version"],
        8
    );
    let (_server, address) = Server::start(&restore_config);
    assert_eq!(whois(address, b"!maut-num,AS64496\n"), "D\n");
    assert!(whois(address, b"!maut-num,AS64497\n").contains("aut-num: AS64497"));
}

#[test]
fn authoritative_organisation_cli_and_http_preserve_authorization_and_references() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"test.redb\"\nsources = [\"TEST\", \"OTHER\"]\nauthoritative_sources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nhttp_authoritative_updates = true\nmax_connections = 4\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n").unwrap();
    let hash = bcrypt::hash_with_salt("organisation secret", 4, [11; 16]).unwrap();
    let snapshot = workspace.path().join("bootstrap.rpsl");
    fs::write(&snapshot, format!("mntner: TEST-MNT\nauth: BCRYPT-PW {hash}\nmnt-by: TEST-MNT\nsource: TEST\n\nperson: Test Contact\nnic-hdl: TC1-TEST\nmnt-by: TEST-MNT\nsource: TEST\n")).unwrap();
    successful(
        cli(&config)
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    let organisation = "organisation: ORG-TEST\norg-name: Documentation Organisation\ncountry: GB\naddress: Example Road\ne-mail: noc@example.net\nadmin-c: TC1-TEST\nmnt-by: TEST-MNT\nsource: TEST\n";
    let file = workspace.path().join("submission.rpsl");
    let submit = |text: &str, delete: bool, password: &str| {
        fs::write(&file, text).unwrap();
        let mut command = cli(&config);
        command
            .args(["update", "--source", "TEST", "--file"])
            .arg(&file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if delete {
            command.arg("--delete");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(password.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let denied = submit(organisation, false, "wrong secret\n");
    assert!(!denied.status.success());
    assert!(!String::from_utf8_lossy(&denied.stderr).contains("wrong secret"));
    let invalid = organisation.replace("TC1-TEST", "MISSING-TEST");
    assert!(
        !submit(
            &format!("{organisation}\n{}", invalid.replace("ORG-TEST", "ORG-BAD")),
            false,
            "organisation secret\n"
        )
        .status
        .success()
    );
    let count = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&count).unwrap()["objects"],
        2
    );
    successful(submit(organisation, false, "organisation secret\n"));
    let contact = "person: Test Contact\nnic-hdl: TC1-TEST\nsource: TEST\n";
    assert!(
        !submit(contact, true, "organisation secret\n")
            .status
            .success()
    );
    let (server, address) = Server::start(&config);
    assert!(whois(address, b"!morganisation,ORG-TEST\n").contains("Documentation Organisation"));
    drop(server);
    let (server, http) = Server::start_listener(&config, "GraphQL listening");
    let request = |text: &str, password: &str| {
        let body = serde_json::json!({"objects":[{"object_text":text}],"passwords":[password]})
            .to_string();
        let raw = whois(http, format!("POST /v1/submit/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
        assert!(!raw.contains(password));
        serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    assert_eq!(
        request(organisation, "wrong secret")["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            &organisation.replace("source: TEST", "source: OTHER"),
            "organisation secret"
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            &organisation.replace("Documentation Organisation", "Updated Organisation"),
            "organisation secret"
        )["summary"]["successful_modify"],
        1
    );
    let body = serde_json::json!({"query": "{ rpslObjects(rpslPk:[\"ORG-TEST\"]) { ... on RPSLOrganisation { orgName country address eMail adminC phone } } }"}).to_string();
    let raw = whois(http, format!("POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
    let data: Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(data.get("errors").is_none(), "{data}");
    assert_eq!(
        data["data"]["rpslObjects"][0],
        serde_json::json!({
            "orgName":"Updated Organisation", "country":["GB"], "address":["Example Road"],
            "eMail":["noc@example.net"], "adminC":["TC1-TEST"], "phone":null
        })
    );
    drop(server);
    successful(submit(organisation, true, "organisation secret\n"));
    successful(submit(contact, true, "organisation secret\n"));
}

#[test]
fn as_block_and_domain_updates_enforce_maintainers_zone_references_and_rollback() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\", \"OTHER\"]\nauthoritative_sources = [\"TEST\", \"OTHER\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nhttp_authoritative_updates = true\nmax_connections = 8\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n").unwrap();
    let snapshot = workspace.path().join("fixture.rpsl");
    let hash = bcrypt::hash_with_salt("domain fixture proof", 4, [17; 16]).unwrap();
    for source in ["TEST", "OTHER"] {
        let mut objects = format!(
            "mntner: OWNER-MNT\nauth: BCRYPT-PW {hash}\nmnt-by: OWNER-MNT\nsource: {source}\n"
        );
        if source == "TEST" {
            objects.push_str("\nperson: Admin Contact\nnic-hdl: CT1-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n\nperson: Zone Contact\nnic-hdl: ZONE-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n");
        }
        fs::write(&snapshot, objects).unwrap();
        successful(
            cli(&config)
                .args([
                    "import",
                    "snapshot",
                    "--source",
                    source,
                    "--authoritative-restore",
                    "--file",
                ])
                .arg(&snapshot)
                .output()
                .unwrap(),
        );
    }
    let block = "as-block: AS64496 - AS64511\nadmin-c: CT1-TEST\ntech-c: CT1-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n";
    let domain = "domain: 2.0.192.in-addr.arpa\nadmin-c: CT1-TEST\ntech-c: CT1-TEST\nzone-c: ZONE-TEST\nnserver: ns.example.invalid\nmnt-by: OWNER-MNT\nsource: TEST\n";
    let submission = workspace.path().join("submission.rpsl");
    let submit = |text: &str, proof: &str, delete: bool| {
        fs::write(&submission, text).unwrap();
        let mut command = cli(&config);
        command
            .args(["update", "--source", "TEST", "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if delete {
            command.arg("--delete");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{proof}\n").as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    for text in [block, domain] {
        assert!(!submit(text, "wrong fixture proof", false).status.success());
    }
    let mixed = format!("{block}\n{}", domain.replace("ZONE-TEST", "MISSING-TEST"));
    assert!(
        !submit(&mixed, "domain fixture proof", false)
            .status
            .success()
    );
    let count = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&count).unwrap()["objects"],
        4
    );
    successful(submit(
        &format!("{block}\n{domain}"),
        "domain fixture proof",
        false,
    ));
    let (server, tcp) = Server::start(&config);
    assert!(whois(tcp, b"!mas-block,AS64496 - AS64511\n").contains("as-block:"));
    assert!(whois(tcp, b"!mdomain,2.0.192.in-addr.arpa\n").contains("zone-c: ZONE-TEST"));
    drop(server);
    let (server, http) = Server::start_listener(&config, "GraphQL listening");
    let request = |method: &str, text: &str, proof: &str| {
        let body = serde_json::json!({"objects":[{"object_text":text}],"passwords":[proof],"delete_reason":"fixture cleanup"}).to_string();
        let raw = whois(http, format!("{method} /v1/submit/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        assert!(raw.starts_with("HTTP/1.1 200 "));
        assert!(!raw.contains(proof) && !raw.contains("$2b$"));
        serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    let zone = "person: Zone Contact\nnic-hdl: ZONE-TEST\nsource: TEST\n";
    assert_eq!(
        request("DELETE", zone, "domain fixture proof")["summary"]["failed"],
        1
    );
    for text in [block, domain] {
        assert_eq!(
            request(
                "POST",
                &text.replace("source: TEST", "source: OTHER"),
                "domain fixture proof"
            )["summary"]["failed"],
            1
        );
        assert_eq!(
            request(
                "POST",
                &text.replace("CT1-TEST", "MISSING-TEST"),
                "domain fixture proof"
            )["summary"]["failed"],
            1
        );
        assert_eq!(
            request(
                "POST",
                &format!("{text}remarks: changed\n"),
                "domain fixture proof"
            )["summary"]["successful_modify"],
            1
        );
        assert_eq!(
            request("DELETE", text, "wrong fixture proof")["summary"]["failed"],
            1
        );
    }
    assert_eq!(
        request("DELETE", domain, "domain fixture proof")["summary"]["successful_delete"],
        1
    );
    assert_eq!(
        request("DELETE", zone, "domain fixture proof")["summary"]["successful_delete"],
        1
    );
    assert_eq!(
        request("DELETE", block, "domain fixture proof")["summary"]["successful_delete"],
        1
    );
    drop(server);
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
}

#[test]
fn authoritative_policy_sets_and_routers_preserve_authorization_and_atomicity() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"test.redb\"\nsources = [\"TEST\", \"OTHER\"]\nauthoritative_sources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nhttp_authoritative_updates = true\nmax_connections = 4\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n").unwrap();
    let snapshot = workspace.path().join("bootstrap.rpsl");
    let parent_hash = bcrypt::hash_with_salt("parent proof", 4, [12; 16]).unwrap();
    let owner_hash = bcrypt::hash_with_salt("owner proof", 4, [13; 16]).unwrap();
    fs::write(&snapshot, format!("mntner: PARENT-MNT\nauth: BCRYPT-PW {parent_hash}\nmnt-by: PARENT-MNT\nsource: TEST\n\nmntner: OWNER-MNT\nauth: BCRYPT-PW {owner_hash}\nmnt-by: OWNER-MNT\nsource: TEST\n\naut-num: AS64496\nmnt-by: PARENT-MNT\nsource: TEST\n\nperson: Set Contact\nnic-hdl: SET1-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n")).unwrap();
    successful(
        cli(&config)
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    let submission = workspace.path().join("submission.rpsl");
    let submit = |body: &str, passwords: &str, delete: bool| {
        fs::write(&submission, body).unwrap();
        let mut command = cli(&config);
        command
            .args(["update", "--source", "TEST", "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if delete {
            command.arg("--delete");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(passwords.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let objects = [
        "filter-set: AS64496:FLTR-TEST\nfilter: AS64496\n",
        "peering-set: AS64496:PRNG-TEST\npeering: AS64496\n",
        "rtr-set: AS64496:RTRS-TEST\nmembers: router.example, AS64496:RTRS-CHILD\nmbrs-by-ref: NONEXISTENT-MNT\n",
    ].map(|head| format!("{head}admin-c: SET1-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n"));
    for object in &objects {
        for proof in ["owner proof\n", "parent proof\n", "wrong proof\n"] {
            let denied = submit(object, proof, false);
            assert!(!denied.status.success());
            assert!(!String::from_utf8_lossy(&denied.stderr).contains(proof.trim()));
        }
    }
    let mixed = format!(
        "{}\n{}",
        objects[0],
        objects[1].replace("SET1-TEST", "MISSING-TEST")
    );
    assert!(
        !submit(&mixed, "owner proof\nparent proof\n", false)
            .status
            .success()
    );
    let count = successful(cli(&config).args(["database", "verify"]).output().unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&count).unwrap()["objects"],
        4
    );
    successful(submit(
        &objects.join("\n"),
        "owner proof\nparent proof\n",
        false,
    ));
    let router = "inet-rtr: router.example\nlocal-as: AS64496\nmember-of: AS64496:RTRS-MISSING\nadmin-c: SET1-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n";
    assert!(!submit(router, "parent proof\n", false).status.success());
    successful(submit(router, "owner proof\n", false));
    let contact = "person: Set Contact\nnic-hdl: SET1-TEST\nsource: TEST\n";
    assert!(!submit(contact, "owner proof\n", true).status.success());
    let (server, tcp) = Server::start(&config);
    for (class, key) in [
        ("filter-set", "FLTR-TEST"),
        ("peering-set", "PRNG-TEST"),
        ("rtr-set", "RTRS-TEST"),
    ] {
        assert!(
            whois(tcp, format!("!m{class},AS64496:{key}\n").as_bytes())
                .contains("mnt-by: OWNER-MNT")
        );
    }
    drop(server);
    let (server, http) = Server::start_listener(&config, "GraphQL listening");
    let request = |method: &str, object: &str, proofs: &[&str]| {
        let body =
            serde_json::json!({"objects":[{"object_text":object}],"passwords":proofs}).to_string();
        let raw = whois(http,format!("{method} /v1/submit/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes());
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
        assert!(!raw.contains("owner proof") && !raw.contains("parent proof"));
        serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    for object in &objects {
        assert_eq!(
            request(
                "POST",
                &object.replace("source: TEST", "source: OTHER"),
                &["owner proof"]
            )["summary"]["failed"],
            1
        );
        assert_eq!(
            request(
                "POST",
                &object.replace("SET1-TEST", "MISSING-TEST"),
                &["owner proof"]
            )["summary"]["failed"],
            1
        );
        assert_eq!(
            request(
                "POST",
                &format!("{object}remarks: HTTP modified\n"),
                &["owner proof"]
            )["summary"]["successful_modify"],
            1
        );
        assert_eq!(
            request("DELETE", object, &["parent proof"])["summary"]["failed"],
            1
        );
        assert_eq!(
            request("DELETE", object, &["owner proof"])["summary"]["successful_delete"],
            1
        );
    }
    assert_eq!(
        request(
            "POST",
            &router.replace("AS64496\n", "bad-asn\n"),
            &["owner proof"]
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            "POST",
            &router.replace("SET1-TEST", "MISSING-TEST"),
            &["owner proof"]
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            "POST",
            &router.replace("source: TEST", "source: OTHER"),
            &["owner proof"]
        )["summary"]["failed"],
        1
    );
    assert_eq!(
        request(
            "POST",
            &format!("{router}alias: alias.example\n"),
            &["owner proof"]
        )["summary"]["successful_modify"],
        1
    );
    assert_eq!(
        request("DELETE", router, &["parent proof"])["summary"]["failed"],
        1
    );
    assert_eq!(
        request("DELETE", router, &["owner proof"])["summary"]["successful_delete"],
        1
    );
    drop(server);
    successful(submit(contact, "owner proof\n", true));
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
}

#[test]
fn maintainer_dummy_updates_rotate_hashes_only_after_authentication() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config,"database = \"test.redb\"\nsources = [\"TEST\"]\nauthoritative_sources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nhttp_authoritative_updates = true\nmax_connections = 4\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n[nrtm3_serve]\nTEST = [\"127.0.0.1\"]\n").unwrap();
    let hash = bcrypt::hash_with_salt("dummy test proof", 4, [15; 16]).unwrap();
    let maintainer = |auth: &str| {
        format!(
            "mntner: TEST-MNT\nadmin-c: TC1-TEST\nupd-to: test@example.net\nauth: BCRYPT-PW {auth}\nmnt-by: TEST-MNT\nsource: TEST\n"
        )
    };
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(
        &snapshot,
        format!(
            "{}\nperson: Test Contact\nnic-hdl: TC1-TEST\nmnt-by: TEST-MNT\nsource: TEST\n",
            maintainer(&hash.to_string())
        ),
    )
    .unwrap();
    successful(
        cli(&config)
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(&snapshot)
            .output()
            .unwrap(),
    );
    let submission = workspace.path().join("submission.rpsl");
    let submit = |body: &str, passwords: &str| {
        fs::write(&submission, body).unwrap();
        let mut child = cli(&config)
            .args(["update", "--source", "TEST", "--file"])
            .arg(&submission)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(passwords.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let dummy = maintainer("DummyValue");
    for (body, proof) in [
        (dummy.clone(), "wrong proof\n"),
        (dummy.clone(), "dummy test proof\nsecond proof\n"),
        (
            format!("{dummy}auth: BCRYPT-PW {hash}\n"),
            "dummy test proof\n",
        ),
        (dummy.replace("TEST-MNT", "NEW-MNT"), "dummy test proof\n"),
        (
            format!(
                "{dummy}\nperson: Bad Contact\naddress: Road\nphone: +1 202 555 0100\ne-mail: noc@example.net\nnic-hdl: BAD-TEST\nmnt-by: MISSING-MNT\nsource: TEST\n"
            ),
            "dummy test proof\n",
        ),
    ] {
        let denied = submit(&body, proof);
        assert!(!denied.status.success());
        assert!(!String::from_utf8_lossy(&denied.stderr).contains("dummy test proof"));
    }
    let history = successful(
        cli(&config)
            .args(["source", "history", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&history).unwrap(),
        serde_json::json!([])
    );
    successful(submit(&dummy, "dummy test proof\n"));
    successful(submit(&dummy, "dummy test proof\ndummy test proof\n"));
    let exported = workspace.path().join("private-export.rpsl");
    successful(
        cli(&config)
            .args([
                "export",
                "snapshot",
                "--source",
                "TEST",
                "--include-sensitive",
                "--file",
            ])
            .arg(&exported)
            .output()
            .unwrap(),
    );
    let raw = fs::read_to_string(&exported).unwrap();
    assert!(!raw.contains("DummyValue") && !raw.contains("dummy test proof"));
    let new_hash = raw
        .lines()
        .find_map(|line| line.strip_prefix("auth: BCRYPT-PW "))
        .unwrap();
    assert!(bcrypt::verify("dummy test proof", new_hash).unwrap());
    assert_ne!(new_hash, hash.to_string());
    let (server, http) = Server::start_listener(&config, "GraphQL listening");
    let request = |proofs: &[&str]| {
        let body =
            serde_json::json!({"objects":[{"object_text":dummy}],"passwords":proofs}).to_string();
        let raw = whois(http,format!("POST /v1/submit/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes());
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
        assert!(
            !raw.contains("dummy test proof")
                && !raw.contains("wrong proof")
                && !raw.contains("$2b$")
        );
        serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    assert_eq!(request(&["wrong proof"])["summary"]["failed"], 1);
    assert_eq!(
        request(&["dummy test proof", "second proof"])["summary"]["failed"],
        1
    );
    assert_eq!(
        request(&["dummy test proof", "dummy test proof"])["summary"]["successful_modify"],
        1
    );
    assert_eq!(
        request(&["dummy test proof"])["summary"]["successful_modify"],
        1
    );
    drop(server);
    let (server, tcp) = Server::start(&config);
    for query in [
        b"!mmntner,TEST-MNT\n".as_slice(),
        b"-g TEST:3:1-LAST\n".as_slice(),
    ] {
        let public = whois(tcp, query);
        assert!(
            !public.contains("auth:")
                && !public.contains("DummyValue")
                && !public.contains("dummy test proof")
                && !public.contains("$2b$")
        );
    }
    drop(server);
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
}

#[test]
fn small_host_controls_fail_before_open_and_persist_with_a_small_cache() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("small.toml");
    let base = "database = \"small.redb\"\nsources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nmax_connections = 4\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n";
    for bad in [
        "database_cache_mib = 0",
        "database_cache_mib = 65537",
        "worker_threads = 0",
        "worker_threads = 65",
        "max_blocking_threads = 0",
        "max_blocking_threads = 257",
        "http_sources = []",
        "http_sources = [\"OTHER\"]",
        "http_sources = [\"TEST\", \"TEST\"]",
        "[node]\nname = \"\"",
        "[node]\nlocation = \"bad\\nlabel\"",
    ] {
        fs::write(&config, format!("{base}{bad}\n")).unwrap();
        let denied = cli(&config).arg("server").output().unwrap();
        assert!(!denied.status.success(), "{bad}");
        assert!(!workspace.path().join("small.redb").exists());
    }
    fs::write(
        &config,
        format!("{base}database_cache_mib = 1\nworker_threads = 1\nmax_blocking_threads = 1\n"),
    )
    .unwrap();
    let fixture = workspace.path().join("fixture.rpsl");
    fs::write(&fixture, "aut-num: AS64496\nsource: TEST\n").unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    for _ in 0..2 {
        let (server, address) = Server::start(&config);
        assert!(whois(address, b"!maut-num,AS64496\n").contains("AS64496"));
        drop(server);
    }
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
}

#[test]
fn snapshot_policies_survive_restart_and_http_source_subset_is_enforced() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\", \"OTHER\"]\nhttp_sources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nmax_connections = 8\nio_timeout_seconds = 2\nmax_snapshot_bytes = 4194304\n").unwrap();
    let fixture = workspace.path().join("fixture.rpsl");
    for (source, marker) in [("TEST", "WEB_ALLOWED"), ("OTHER", "TCP_ONLY")] {
        let mut text = format!("aut-num: AS64496\ndescr: {marker}\nsource: {source}\n\n");
        if source == "TEST" {
            text.push_str(&format!(
                "filter-set: FLTR-LARGE\nfilter: {}\nsource: TEST\n",
                "AS64496 OR ".repeat(100_000) + "AS64497"
            ));
        }
        fs::write(&fixture, text).unwrap();
        successful(
            cli(&config)
                .args(["import", "snapshot", "--source", source, "--file"])
                .arg(&fixture)
                .output()
                .unwrap(),
        );
    }
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
    let (server, address) = Server::start(&config);
    assert!(whois(address, b"-s OTHER AS64496\n").contains("TCP_ONLY"));
    drop(server);
    let (server, address) = Server::start_listener(&config, "GraphQL listening");
    let result = whois(
        address,
        b"GET /v1/whois/?q=AS64496 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(result.contains("WEB_ALLOWED") && !result.contains("TCP_ONLY"));
    let denied = whois(address, b"GET /v1/whois/?q=-s%20OTHER%20AS64496 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(denied.starts_with("HTTP/1.1 400 ") && !denied.contains("TCP_ONLY"));
    for query in [
        "{ databaseStatus { source } }",
        "{ databaseStatus(sources:[\"OTHER\"]) { source } }",
    ] {
        let body = serde_json::json!({"query":query}).to_string();
        let raw = whois(address, format!("POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        let value: Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
        if query.contains("OTHER") {
            assert!(value.get("errors").is_some());
        } else {
            assert_eq!(
                value["data"]["databaseStatus"],
                serde_json::json!([{"source":"TEST"}])
            );
        }
    }
    drop(server);
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
}

#[test]
fn full_standard_class_profile_is_queryable_after_restart() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nmax_connections = 8\ndatabase_cache_mib = 8\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n").unwrap();
    // Synthetic identities cover the full pinned IRRd class inventory, including
    // classes absent from a routing-only snapshot. This tests mirror/query
    // coverage; these deliberately minimal objects are not update-validation fixtures.
    let cases = [
        (
            "as-block",
            "AS64496 - AS64511",
            "as-block: AS64496 - AS64511",
        ),
        (
            "as-set",
            "AS64496:AS-EXAMPLE",
            "as-set: AS64496:AS-EXAMPLE\nmembers: AS64497",
        ),
        ("aut-num", "AS64496", "aut-num: AS64496"),
        (
            "domain",
            "2.0.192.IN-ADDR.ARPA",
            "domain: 2.0.192.in-addr.arpa",
        ),
        (
            "filter-set",
            "FLTR-EXAMPLE",
            "filter-set: FLTR-EXAMPLE\nfilter: AS64496",
        ),
        (
            "inet-rtr",
            "R1.EXAMPLE.INVALID",
            "inet-rtr: r1.example.invalid",
        ),
        ("inet6num", "2001:DB8::/32", "inet6num: 2001:db8::/32"),
        (
            "inetnum",
            "192.0.2.0 - 192.0.2.255",
            "inetnum: 192.0.2.0 - 192.0.2.255",
        ),
        ("irt", "IRT-EXAMPLE", "irt: IRT-EXAMPLE"),
        ("key-cert", "PGPKEY-A1B2C3D4", "key-cert: PGPKEY-A1B2C3D4"),
        (
            "mntner",
            "EXAMPLE-MNT",
            "mntner: EXAMPLE-MNT\nauth: BCRYPT-PW synthetic-full-profile-proof",
        ),
        ("organisation", "ORG-EX1-TEST", "organisation: ORG-EX1-TEST"),
        (
            "peering-set",
            "PRNG-EXAMPLE",
            "peering-set: PRNG-EXAMPLE\npeering: AS64496",
        ),
        ("person", "EX1-TEST", "person:\nnic-hdl: EX1-TEST"),
        ("role", "NOC1-TEST", "role:\nnic-hdl: NOC1-TEST"),
        (
            "route",
            "192.0.2.0/24AS64496",
            "route: 192.0.2.0/24\norigin: AS64496",
        ),
        (
            "route-set",
            "RS-EXAMPLE",
            "route-set: RS-EXAMPLE\nmembers: 192.0.2.0/24",
        ),
        (
            "route6",
            "2001:DB8::/32AS64496",
            "route6: 2001:db8::/32\norigin: AS64496",
        ),
        (
            "rtr-set",
            "RTRS-EXAMPLE",
            "rtr-set: RTRS-EXAMPLE\nmembers: r1.example.invalid",
        ),
    ];
    let fixture = workspace.path().join("full.rpsl");
    fs::write(
        &fixture,
        cases
            .iter()
            .map(|(_, _, text)| format!("{text}\nsource: TEST\n\n"))
            .collect::<String>(),
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    for _ in 0..2 {
        let (server, tcp) = Server::start_listener(&config, "WHOIS listening");
        for (class, key, text) in cases {
            let answer = whois(tcp, format!("!m{class},{key}\n").as_bytes());
            assert!(
                answer.contains(text.lines().next().unwrap()),
                "{class}: {answer}"
            );
            assert!(!answer.contains("synthetic-full-profile-proof"));
        }
        drop(server);
        let (server, http) = Server::start_listener(&config, "GraphQL listening");
        for (class, key, _) in cases {
            let response = whois(http, format!("GET /v1/irr/objects?source=TEST&class={class} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes());
            assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
            let value: Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(value["keys"], serde_json::json!([key]), "{class}");
        }
        for batch in cases.chunks(8) {
            let body = serde_json::json!({
            "query": "query($keys:[String!]) { rpslObjects(sources:[\"TEST\"],rpslPk:$keys) { objectClass rpslPk objectText ... on RPSLPerson { person } ... on RPSLRole { role } } }",
            "variables": {"keys": batch.iter().map(|(_, key, _)| key).collect::<Vec<_>>()}
        }).to_string();
            let response = whois(http, format!("POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            let value: Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert!(value.get("errors").is_none(), "{value}");
            let rows = value["data"]["rpslObjects"].as_array().unwrap();
            assert_eq!(rows.len(), batch.len());
            for (class, key, text) in batch {
                let row = rows
                    .iter()
                    .find(|row| row["objectClass"] == *class)
                    .unwrap();
                assert_eq!(row["rpslPk"], *key);
                if matches!(*class, "person" | "role") {
                    assert_eq!(row[*class], "");
                }
                assert!(
                    row["objectText"]
                        .as_str()
                        .unwrap()
                        .contains(text.lines().next().unwrap())
                );
            }
            assert!(!response.contains("synthetic-full-profile-proof"));
        }
        drop(server);
    }
    let checked: Value = serde_json::from_slice(&successful(
        cli(&config).args(["database", "verify"]).output().unwrap(),
    ))
    .unwrap();
    assert_eq!(checked["objects"], 19);
    let exported = workspace.path().join("public.rpsl");
    successful(
        cli(&config)
            .args([
                "export", "snapshot", "--source", "TEST", "--public", "--file",
            ])
            .arg(&exported)
            .output()
            .unwrap(),
    );
    let public = fs::read_to_string(exported).unwrap();
    assert!(!public.contains("synthetic-full-profile-proof"));
    for (_, _, text) in cases {
        assert!(public.contains(text.lines().next().unwrap()));
    }
}

#[test]
fn raw_http_whois_scope_is_independent_of_structured_queries_and_browse() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\", \"OTHER\"]\nhttp_sources = [\"TEST\"]\nhttp_whois_sources = [\"TEST\", \"OTHER\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nmax_connections = 8\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n").unwrap();
    let fixture = workspace.path().join("fixture.rpsl");
    for source in ["TEST", "OTHER"] {
        fs::write(&fixture, format!("as-set: AS-A\nmembers: AS64496\nsource: {source}\n\nas-set: AS-B\nmembers: AS64497\nsource: {source}\n\nmntner: PROFILE-MNT\nauth: BCRYPT-PW private-index-proof\nsource: {source}\n")).unwrap();
        successful(
            cli(&config)
                .args(["import", "snapshot", "--source", source, "--file"])
                .arg(&fixture)
                .output()
                .unwrap(),
        );
    }
    let (_server, address) = Server::start_listener(&config, "GraphQL listening");
    let get = |path: &str| {
        whois(
            address,
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
    };
    let raw = get("/v1/whois/?q=-s%20OTHER%20AS-A");
    assert!(raw.starts_with("HTTP/1.1 200 ") && raw.contains("source: OTHER"));
    for path in [
        "/v1/irr/objects?source=OTHER&class=as-set",
        "/v1/irr/objects?source=TEST&class=auth",
        "/v1/irr/objects?source=TEST&class=as-set&limit=101",
        "/v1/irr/objects?source=TEST&class=as-set&after=%00",
    ] {
        assert!(get(path).starts_with("HTTP/1.1 400 "), "{path}");
    }
    let members = get("/v1/whois/?q=!iAS-A,1&sources=OTHER");
    assert!(members.starts_with("HTTP/1.1 200 ") && members.contains("AS64496"));
    assert!(get("/v1/whois/?q=!iAS-A,1&sources=UNAVAILABLE").starts_with("HTTP/1.1 400 "));
    let json_get = |path: &str| -> Value {
        let raw = get(path);
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
        serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
    };
    let maintainers = json_get("/v1/irr/objects?source=TEST&class=mntner");
    assert_eq!(maintainers["keys"], serde_json::json!(["PROFILE-MNT"]));
    assert!(!maintainers.to_string().contains("private-index-proof"));
    assert!(get("/v1/irr/objects?source=OTHER&class=mntner").starts_with("HTTP/1.1 400 "));
    let first = json_get("/v1/irr/objects?source=TEST&class=as-set&limit=1");
    assert_eq!(first["keys"], serde_json::json!(["AS-A"]));
    assert_eq!(first["next_cursor"], "AS-A");
    let second = json_get("/v1/irr/objects?source=TEST&class=as-set&limit=1&after=AS-A");
    assert_eq!(second["keys"], serde_json::json!(["AS-B"]));
    assert!(second["next_cursor"].is_null());
    for path in [
        "/v1/rpki/status",
        "/v1/rpki/validate?prefix=192.0.2.0%2F24&origin=AS64496",
    ] {
        assert!(get(path).starts_with("HTTP/1.1 404 "));
    }
    let counts = json_get("/ui/mirrors.json");
    assert_eq!(counts["class_counts"]["TEST"]["as-set"], 2);
    let body =
        serde_json::json!({"query":"{ rpslObjects(sources:[\"OTHER\"]) { rpslPk } }"}).to_string();
    let response=whois(address,format!("POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes());
    let value: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(value.get("errors").is_some());
}

#[test]
fn bundled_dashboard_serves_local_assets_metadata_and_uses_the_private_api() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("dashboard.toml");
    fs::write(&config,"database = \"registry.redb\"\nsources = [\"TEST\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nmax_connections = 4\nworker_threads = 1\nmax_blocking_threads = 2\ndatabase_cache_mib = 8\nio_timeout_seconds = 2\nmax_snapshot_bytes = 65536\n[node]\nname = \"Test node\"\nlocation = \"Documentation lab\"\nsponsor = \"Synthetic fixture\"\n").unwrap();
    let fixture = workspace.path().join("fixture.rpsl");
    fs::write(
        &fixture,
        "mntner: TEST-MNT\nauth: BCRYPT-PW hidden-dashboard-proof\nsource: TEST\n",
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    let (server, address) = Server::start_listener(&config, "GraphQL listening");
    let get = |path: &str, host: &str| {
        whois(
            address,
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
    };
    let html = get("/ui/", "localhost");
    assert!(html.starts_with("HTTP/1.1 200 ") && html.contains("Registry explorer"));
    assert!(html.contains("script-src 'self'") && html.contains("frame-ancestors 'none'"));
    for (path, content_type) in [
        ("/ui/style.css", "text/css"),
        ("/ui/app.js", "text/javascript"),
        ("/ui/network.json", "application/json"),
    ] {
        let result = get(path, "localhost");
        assert!(
            result.starts_with("HTTP/1.1 200 ") && result.contains(content_type),
            "{path}"
        );
        let denied = get(path, "untrusted.example");
        assert!(denied.starts_with("HTTP/1.1 400 "), "{path}");
    }
    let data = get("/ui/meta.json", "localhost");
    let meta: Value = serde_json::from_str(data.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(meta["mode"], "live");
    assert_eq!(meta["sources"], serde_json::json!(["TEST"]));
    assert_eq!(meta["node"]["name"], "Test node");
    assert_eq!(meta["node"]["sponsor"], "Synthetic fixture");
    assert_eq!(meta["database_cache_mib"], 8);
    assert_eq!(meta["max_connections"], 4);
    assert!(!data.contains("registry.redb") && !data.contains("hidden-dashboard-proof"));
    assert!(get("/ui", "localhost").starts_with("HTTP/1.1 308 "));
    assert!(get("/", "localhost").starts_with("HTTP/1.1 302 "));
    assert!(get("/ui/sample-results.json", "localhost").starts_with("HTTP/1.1 404 "));
    let query = get("/v1/whois/?q=TEST-MNT", "localhost");
    assert!(
        query.contains("TEST-MNT")
            && !query.contains("hidden-dashboard-proof")
            && !query.contains("auth:")
    );
    drop(server);
}

#[test]
fn filtered_nrtm3_survives_restart_and_keeps_excluded_classes_out_of_public_queries() {
    let workspace = tempfile::tempdir().unwrap();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = upstream.local_addr().unwrap();
    let config = workspace.path().join("irrd.toml");
    fs::write(
        &config,
        format!(
            r#"database = "registry.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
http_listen = "127.0.0.1:0"
max_connections = 8
io_timeout_seconds = 2
max_snapshot_bytes = 8192
[nrtm3_sync.TEST]
endpoint = "{endpoint}"
interval_seconds = 1
base_serial = 41
object_class_filter = ["aut-num", "route"]
"#
        ),
    )
    .unwrap();
    let snapshot = workspace.path().join("snapshot.rpsl");
    fs::write(
        &snapshot,
        "aut-num: AS64496\nsource: TEST\n\nroute: 192.0.2.0/24\norigin: AS64496\nsource: TEST\n",
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(snapshot)
            .output()
            .unwrap(),
    );
    upstream.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        for trailer in ["OTHER", "TEST"] {
            let mut stream = loop {
                match upstream.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("accepting mirror: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(IO_DEADLINE)).unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            assert_eq!(request, "-g TEST:3:42-LAST\n");
            write!(stream,"%START Version: 3 TEST 42-47\n\nADD 43\n\nmntner: PRIVATE-MNT\nauth: BCRYPT-PW private-fixture-only\nsource: TEST\n\nADD 45\n\nroute: 198.51.100.0/24\norigin: AS64496\nsource: TEST\n\nDEL 46\n\nroute: 192.0.2.0/24\norigin: AS64496\nsource: TEST\n\n%END {trailer}\n").unwrap();
        }
    });
    let (server, http) = Server::start_listener(&config, "GraphQL listening");
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let status = whois(
            http,
            b"GET /ui/mirrors.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        if status.contains("\"serial\":47") {
            break;
        }
        assert!(Instant::now() < deadline, "mirror did not commit: {status}");
        thread::sleep(Duration::from_millis(50));
    }
    let request = |q: &str| {
        whois(
            http,
            format!(
                "GET /v1/whois/?q={q} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
    };
    assert!(request("-x%20198.51.100.0%2F24").contains("198.51.100.0/24"));
    assert!(request("-x%20192.0.2.0%2F24").starts_with("HTTP/1.1 204 "));
    assert!(request("PRIVATE-MNT").starts_with("HTTP/1.1 204 "));
    let body = r#"{"query":"{ databaseStatus { serialNewestMirror localJournalKept synchronisedSerials } }"}"#;
    let status=whois(http,format!("POST /graphql/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes());
    let status: Value = serde_json::from_str(status.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        status["data"]["databaseStatus"][0],
        serde_json::json!({"serialNewestMirror":47,"localJournalKept":false,"synchronisedSerials":false})
    );
    drop(server);
    peer.join().unwrap();
    let serial = successful(
        cli(&config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&serial).unwrap()["serial"],
        47
    );
    successful(cli(&config).args(["database", "verify"]).output().unwrap());
    let exported = workspace.path().join("public.rpsl");
    successful(
        cli(&config)
            .args([
                "export", "snapshot", "--source", "TEST", "--public", "--file",
            ])
            .arg(&exported)
            .output()
            .unwrap(),
    );
    let text = fs::read_to_string(exported).unwrap();
    assert!(!text.contains("PRIVATE-MNT") && !text.contains("private-fixture-only"));
    let backup = workspace.path().join("filtered.backup");
    successful(
        cli(&config)
            .args(["database", "backup", "--file"])
            .arg(&backup)
            .output()
            .unwrap(),
    );
    let restored = workspace.path().join("restored.toml");
    fs::write(
        &restored,
        fs::read_to_string(&config)
            .unwrap()
            .replace("registry.redb", "restored.redb"),
    )
    .unwrap();
    successful(
        cli(&restored)
            .args(["database", "restore", "--file"])
            .arg(&backup)
            .output()
            .unwrap(),
    );
    successful(
        cli(&restored)
            .args(["database", "verify"])
            .output()
            .unwrap(),
    );
    let restored_status = successful(
        cli(&restored)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&restored_status).unwrap()["serial"],
        47
    );
    let (server, tcp) = Server::start(&restored);
    assert!(whois(tcp, b"!r198.51.100.0/24\n").contains("198.51.100.0/24"));
    assert_eq!(whois(tcp, b"!mmntner,PRIVATE-MNT\n"), "D\n");
    drop(server);
}

#[test]
fn public_http_admission_preserves_queries_and_rejects_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("irrd.toml");
    let base = r#"database = "registry.redb"
sources = ["TEST"]
listen = "127.0.0.1:0"
http_listen = "127.0.0.1:0"
http_public_queries = true
max_connections = 4
io_timeout_seconds = 2
max_snapshot_bytes = 65536
"#;
    fs::write(&config, base).unwrap();
    let fixture = workspace.path().join("fixture.rpsl");
    fs::write(
        &fixture,
        (0..54)
            .map(|n| format!("route6: 2001:db8:{n:x}::/48\norigin: AS64496\nsource: TEST\n\n"))
            .collect::<String>(),
    )
    .unwrap();
    successful(
        cli(&config)
            .args(["import", "snapshot", "--source", "TEST", "--file"])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    let (server, address) = Server::start_listener(&config, "GraphQL listening");
    let get = |path: &str, extra: &str| {
        whois(
            address,
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{extra}Connection: close\r\n\r\n")
                .as_bytes(),
        )
    };
    let forwarded = "X-RIRRd-Client: 192.0.2.1\r\n";
    let mut limited = 0;
    for _ in 0..40 {
        let reply = get("/v1/whois/?q=%216AS64496", forwarded);
        if reply.starts_with("HTTP/1.1 200") {
            assert_eq!(
                reply
                    .split_once("\r\n\r\n")
                    .unwrap()
                    .1
                    .split_whitespace()
                    .count(),
                54
            );
            assert!(reply.to_lowercase().contains("server-timing: rirrd;dur="));
        } else {
            assert!(reply.starts_with("HTTP/1.1 429"), "{reply}");
            limited += 1;
        }
    }
    assert!(limited > 0);
    assert!(get("/v1/whois/?q=%216AS64496", "").starts_with("HTTP/1.1 200"));
    assert!(
        get("/v1/whois/?q=%216AS64496", "X-RIRRd-Client: malformed\r\n")
            .starts_with("HTTP/1.1 400")
    );
    assert!(get("/ui/http-stats.json", forwarded).starts_with("HTTP/1.1 404"));
    assert!(get("/v1/submit/", forwarded).starts_with("HTTP/1.1 404"));
    let metrics = get("/ui/http-stats.json", "");
    let report: Value = serde_json::from_str(metrics.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(report["requests"], 43);
    assert_eq!(report["rate_limited"], limited);
    assert_eq!(report["successful_sample_count"], 40 - limited);
    drop(server);
    fs::write(
        &config,
        format!("{base}http_authoritative_updates = true\n"),
    )
    .unwrap();
    let output = cli(&config).args(["config", "check"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("http_public_queries"));
}
