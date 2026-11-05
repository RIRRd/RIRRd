//! Exercise new authoritative classes through the shipped CLI and HTTP service.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

const OWNER: &str = "allocation owner fixture";
const PARENT: &str = "allocation parent fixture";

fn cli(config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-irrd"));
    command.arg("--config").arg(config).env("RUST_LOG", "info");
    command
}

fn success(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn update(config: &Path, text: &str, passwords: &[&str], delete: bool) -> Output {
    let path = config.with_extension("rpsl");
    fs::write(&path, text).unwrap();
    let source = if text.contains("source: OTHER") {
        "OTHER"
    } else {
        "TEST"
    };
    let mut command = cli(config);
    command
        .args(["update", "--source", source, "--file"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if delete {
        command.arg("--delete");
    }
    let mut child = command.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{}", passwords.join("\n")).unwrap();
    let output = child.wait_with_output().unwrap();
    for secret in [OWNER, PARENT, "wrong fixture"] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    }
    output
}

fn export(config: &Path) -> Vec<u8> {
    let path = config.with_extension("export");
    let _ = fs::remove_file(&path);
    success(
        cli(config)
            .args([
                "export", "snapshot", "--source", "TEST", "--public", "--file",
            ])
            .arg(&path)
            .output()
            .unwrap(),
    );
    fs::read(path).unwrap()
}

struct Server {
    child: Child,
    reader: Option<JoinHandle<()>>,
}
impl Server {
    fn start(config: &Path) -> (Self, SocketAddr) {
        let mut child = cli(config)
            .arg("server")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(log) = serde_json::from_str::<Value>(&line)
                    && log["fields"]["message"] == "GraphQL listening"
                {
                    let _ =
                        sender.send(log["fields"]["address"].as_str().unwrap().parse().unwrap());
                }
            }
        });
        let mut server = Self {
            child,
            reader: Some(reader),
        };
        let address = receiver
            .recv_timeout(Duration::from_secs(15))
            .unwrap_or_else(|error| {
                let state = server.child.try_wait();
                panic!("server did not start: {error}; {state:?}")
            });
        (server, address)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn http(address: SocketAddr, method: &str, path: &str, payload: Value) -> Value {
    let body = serde_json::to_string(&payload).unwrap();
    let mut connection = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    connection
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(connection, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    for secret in [OWNER, PARENT, "wrong fixture"] {
        assert!(!response.contains(secret));
    }
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn submit(address: SocketAddr, objects: &[&str], passwords: &[&str], delete: bool, result: &str) {
    let body = json!({"objects": objects.iter().map(|text| json!({"object_text":text})).collect::<Vec<_>>(),"passwords":passwords});
    let report = http(
        address,
        if delete { "DELETE" } else { "POST" },
        "/v1/submit/",
        body,
    );
    assert_eq!(report["summary"][result], objects.len(), "{report}");
}

fn setup(path: &Path) -> PathBuf {
    let config = path.join("irrd.toml");
    fs::write(&config, "database = \"registry.redb\"\nsources = [\"TEST\", \"OTHER\"]\nauthoritative_sources = [\"TEST\", \"OTHER\"]\nlisten = \"127.0.0.1:0\"\nhttp_listen = \"127.0.0.1:0\"\nhttp_authoritative_updates = true\nmax_connections = 8\nio_timeout_seconds = 5\nmax_snapshot_bytes = 65536\n").unwrap();
    let fixture = path.join("bootstrap.rpsl");
    let mut text = String::new();
    for (name, password) in [("PARENT-MNT", PARENT), ("OWNER-MNT", OWNER)] {
        let hash = bcrypt::hash(password, 4).unwrap();
        text.push_str(&format!("mntner: {name}\nadmin-c: BOOT-TEST\nupd-to: test@example.invalid\nauth: BCRYPT-PW {hash}\nmnt-by: {name}\nsource: TEST\n\n"));
    }
    text.push_str("person: Bootstrap Contact\naddress: Documentation Road\nphone: +1 202 555 0100\ne-mail: test@example.invalid\nnic-hdl: BOOT-TEST\nmnt-by: PARENT-MNT\nsource: TEST\n");
    fs::write(&fixture, text).unwrap();
    success(
        cli(&config)
            .args([
                "import",
                "snapshot",
                "--source",
                "TEST",
                "--authoritative-restore",
                "--file",
            ])
            .arg(fixture)
            .output()
            .unwrap(),
    );
    config
}

#[test]
fn allocations_and_irt_authorize_atomically_and_protect_parent_and_contact_state() {
    let directory = tempfile::tempdir().unwrap();
    let config = setup(directory.path());
    let contact = "person: Allocation Contact\naddress: Documentation Road\nphone: +1 202 555 0100\ne-mail: contact@example.invalid\nnic-hdl: ALLOC-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n";
    let fields = "netname: Documentation\ncountry: GB\nadmin-c: ALLOC-TEST\ntech-c: ALLOC-TEST\nstatus: ASSIGNED PA\nmnt-by: PARENT-MNT\nsource: TEST\n";
    let v4 = format!("inetnum: 198.18.16.0 - 198.18.16.255\n{fields}");
    let v6 = format!("inet6num: 2001:db8:abcd::/48\n{fields}");
    let irt = "irt: IRT-EXAMPLE\naddress: Documentation Road\ne-mail: contact@example.invalid\nabuse-mailbox: abuse@example.invalid\nadmin-c: ALLOC-TEST\nmnt-by: OWNER-MNT\nsource: TEST\n";
    let batch = format!("{contact}\n{v4}\n{v6}\n{irt}");
    assert!(!update(&config, &batch, &[OWNER], false).status.success());
    success(update(&config, &batch, &[OWNER, PARENT], false));
    let before = export(&config);
    for text in [
        v4.replace("source: TEST", "source: OTHER"),
        v6.replace("ALLOC-TEST", "MISSING-TEST"),
        irt.replace("abuse-mailbox: abuse@example.invalid\n", ""),
    ] {
        assert!(
            !update(&config, &text, &[OWNER, PARENT], false)
                .status
                .success()
        );
    }
    let failed_batch = format!(
        "{v4}remarks: must roll back\n\n{}",
        irt.replace("abuse-mailbox: abuse@example.invalid\n", "")
    );
    assert!(
        !update(&config, &failed_batch, &[OWNER, PARENT], false)
            .status
            .success()
    );
    assert!(!update(&config, contact, &[OWNER], true).status.success());
    assert_eq!(export(&config), before);
    // Newly created allocations immediately govern route creation, in each family.
    let routes = "route: 198.18.16.0/24\norigin: AS64496\nmnt-by: OWNER-MNT\nsource: TEST\n\nroute6: 2001:db8:abcd::/48\norigin: AS64496\nmnt-by: OWNER-MNT\nsource: TEST\n";
    assert!(!update(&config, routes, &[OWNER], false).status.success());
    success(update(&config, routes, &[OWNER, PARENT], false));
    success(update(&config, routes, &[OWNER], true));
    assert_eq!(export(&config), before);

    let (server, address) = Server::start(&config);
    let moved = v4.replace("mnt-by: PARENT-MNT", "mnt-by: OWNER-MNT");
    submit(address, &[&moved], &[OWNER], false, "failed");
    submit(address, &[&moved], &[PARENT], false, "failed");
    submit(
        address,
        &[&moved],
        &[OWNER, PARENT],
        false,
        "successful_modify",
    );
    submit(address, &[&moved], &[PARENT], true, "failed");
    let changed_v6 = format!("{v6}remarks: HTTP change\n");
    let changed_irt = format!("{irt}remarks: HTTP change\n");
    submit(
        address,
        &[&changed_v6, &changed_irt],
        &[OWNER, PARENT],
        false,
        "successful_modify",
    );
    let failed_irt = irt.replace("abuse-mailbox: abuse@example.invalid\n", "");
    submit(
        address,
        &[&format!("{moved}remarks: must roll back\n"), &failed_irt],
        &[OWNER, PARENT],
        false,
        "failed",
    );
    submit(address, &[contact], &[OWNER], true, "failed");
    let query = http(
        address,
        "POST",
        "/graphql/",
        json!({"query":"{ rpslObjects(sources:[\"TEST\"],objectClass:[\"inetnum\",\"inet6num\",\"irt\"],rpslPk:[\"198.18.16.0 - 198.18.16.255\",\"2001:DB8:ABCD::/48\",\"IRT-EXAMPLE\"]){rpslPk objectClass objectText} }"}),
    );
    assert!(query.get("errors").is_none(), "{query}");
    let rows = query["data"]["rpslObjects"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        !serde_json::to_string(rows)
            .unwrap()
            .contains("must roll back")
    );
    submit(
        address,
        &[&moved, &changed_v6, &changed_irt, contact],
        &[OWNER, PARENT],
        true,
        "successful_delete",
    );
    drop(server);
    let verified: Value = serde_json::from_slice(&success(
        cli(&config).args(["database", "verify"]).output().unwrap(),
    ))
    .unwrap();
    assert_eq!(verified["objects"], 3);
    let status: Value = serde_json::from_slice(&success(
        cli(&config)
            .args(["source", "status", "--source", "TEST"])
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(status["local_serial"], 15);
}
