# Self-host RIRRd beta

RIRRd is a Rust Internet Routing Registry daemon with embedded storage, a bundled
console and an optional embedded RPKI validator. It runs independently of the
community service at [irrd.tools](https://irrd.tools/).

Version **0.1.0-beta.2** is for evaluation. Full IRRd parity and production
qualification are incomplete. Read [beta status](/docs/beta/) before deployment.
The project is MIT licensed. The canonical repository is RIRRd/RIRRd on GitHub.
Use a tagged beta release for a reproducible evaluation.

## Build and try a local fixture

On Linux, install stable Rust, a C compiler/linker, pkg-config and CA certificates.
Build from the source checkout:

```sh
git clone https://github.com/RIRRd/RIRRd.git
cd RIRRd
cargo build --locked --release
./target/release/rust-irrd --config examples/irrd.toml config check
./target/release/rust-irrd --config examples/irrd.toml import snapshot \
  --source EXAMPLE --file examples/example.rpsl
./target/release/rust-irrd --config examples/irrd.toml database verify
./target/release/rust-irrd --config examples/irrd.toml server
```

The synthetic fixture listens on local port 4343. In another terminal:

```sh
whois -h 127.0.0.1 -p 4343 '!gAS64496'
whois -h 127.0.0.1 -p 4343 '!6AS64496'
```

These are **local evaluation commands**; public-node commands use `irrd.tools`.
To enable the bundled console, add `http_listen = "127.0.0.1:8080"` at the top
level of the example configuration, restart and open `http://127.0.0.1:8080/ui/`.
For a remote host use your own authenticated management tunnel. The native
HTTP listener deliberately binds loopback; public deployments use a reverse proxy.

## Keep a small deployment small

Choose the sources you need and set an explicit database cache budget. Start
with the source repository's `examples/small-host/irrd.toml`; it uses a bounded
cache and connection budget. Cache size is not a whole-process memory limit.
Measure import peaks, steady queries and validation refreshes separately.

Stop the serving process before offline imports, exports, verification or
compaction: the database has exclusive process ownership. The in-process NRTMv3
scheduler can apply verified source updates without stopping query listeners.
Keep database files, mirror snapshots, credentials and validator caches outside Git.

## Add RPKI when needed

Omit `[rpki]` for an IRR-only instance. To enable the embedded validator, configure
an absolute cache path owned by the service account:

```toml
[rpki]
cache_dir = "/var/lib/rust-irrd/rpki"
rtr_listen = "127.0.0.1:3323"
refresh_seconds = 600
expire_seconds = 3600
validation_threads = 2
```

RPKI collection adds memory, disk and cryptographic work. The current beta uses
Routinator/rpki-rs in process and RRDP transport; rsync-only repositories are
outside its coverage. Read [RPKI behavior](/docs/rpki/) and use the status endpoint
before consuming validation results. Unavailable data is not a NOT_FOUND verdict.

## Connect routing tools

See [BIRD and FRR examples](/docs/routers/) for public bgpq4 queries and safe
filter generation. IRR policy filtering and RPKI route-origin validation answer
different questions. The public service exposes HTTPS origin checks; local RTR
is available to operators on their own host or authenticated management network.
