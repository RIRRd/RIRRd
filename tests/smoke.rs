use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
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
                if log["fields"]["message"] == "WHOIS listening"
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
    assert_eq!(
        whois(address, b"!r2001:db8::/32\n"),
        format!("A{}\n{route}C\n", route.len())
    );
    assert_eq!(
        whois(address, b"TEST-MNT\r\n"),
        "mntner: TEST-MNT\r\nsource: TEST\r\n\r\n"
    );
    assert_eq!(
        whois(address, b"-i origin AS64496\r\n"),
        "% Invalid or unsupported query\r\n"
    );
}
