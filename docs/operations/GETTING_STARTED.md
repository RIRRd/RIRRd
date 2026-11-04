# Self-host RIRRd beta

RIRRd is a Rust Internet Routing Registry daemon with embedded storage, a bundled
console. It runs independently of the
community service at [irrd.tools](https://irrd.tools/).

Version **0.1.0-beta.2** is for evaluation. Full IRRd parity and production
qualification are incomplete. Read [beta status](/docs/beta/) before deployment.
The project is MIT licensed. The canonical repository is RIRRd/RIRRd on GitHub.
Use a tagged beta release for a reproducible evaluation.

## Linux release packages

Download the matching x86-64 or ARM64 DEB/RPM from
[GitHub Releases](https://github.com/RIRRd/RIRRd/releases). The beta release
workflow builds on Debian 12, tests native packages and publishes `SHA256SUMS`.
Packages preserve the operator configuration and require explicit service startup.

Use the [installer attached to beta.2](https://github.com/RIRRd/RIRRd/releases/download/v0.1.0-beta.2/install.sh)
or the current source checkout. It selects the architecture and verifies the
archive against the release checksum manifest. The attached installer includes
the post-publication GitHub filename correction; the initial beta.2 tag's copy
predates that correction.

```sh
# Download and verify only; no root access or service change.
bash tools/install.sh --version 0.1.0-beta.2 --download-only /tmp/rirrd-package
# Or install with your distribution package manager.
sudo bash tools/install.sh --version 0.1.0-beta.2
```

Configure `/etc/rust-irrd/irrd.toml`, import a source snapshot while the daemon is
stopped, run `config check` and `database verify`, then start
`rust-irrd.service` explicitly. Keep HTTP on loopback behind your own proxy.

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
Measure import peaks, steady queries and mirror updates separately.

Stop the serving process before offline imports, exports, verification or
compaction: the database has exclusive process ownership. The in-process NRTMv3
scheduler can apply verified source updates without stopping query listeners.
Keep database files, mirror snapshots, and credentials outside Git.

## Connect routing tools

See [BIRD and FRR examples](/docs/routers/) for public bgpq4 queries and safe
filter generation. RIRRd is focused on IRR data and queries. Run a separate RPKI
validator if your routing policy also needs cryptographic origin validation.
