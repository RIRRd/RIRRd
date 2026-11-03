# IRRd 4.5.4 WHOIS test oracle

This Docker Compose stack starts the pinned upstream daemon with PostgreSQL,
Redis and the repository's synthetic RPSL fixture. WHOIS listens on host
loopback only. The HTTP listener stays inside the reference container. The
database credentials are local test values. Each project gets its own
PostgreSQL volume and Compose network.

The image uses the IRRd 4.5.4 PyPI wheel with a verified SHA-256. All 151
packaged Python source files matched the source at
`8e8080dab744a348b7d0e6efcb53f403301dd075` after normalizing checkout line
endings. Base images are pinned to multi-architecture manifest digests.
Transitive Python and Debian dependencies are resolved at build time. For
recorded benchmark runs, record the built image ID and package versions.
[Upstream release](https://github.com/irrdnet/irrd/releases/tag/v4.5.4) ·
[PyPI wheel](https://pypi.org/project/irrd/4.5.4/#files)

From the repository root, confirm that Docker is running Linux containers,
then start the stack using a Compose project name dedicated to this oracle.

```sh
docker info --format '{{.OSType}}'
docker compose -p irrd-454-oracle -f tools/oracle/compose.yaml up --build -d --wait --wait-timeout 120
```

The reference container applies database migrations, loads the fixture, then
starts WHOIS. Its health check verifies that a fixture query returns an IRRd
success header. Use `IRRD_ORACLE_PORT` to select another loopback host port
if 8043 is already in use. Update the comparison command's reference port as
well.

Create a TOML configuration in a temporary directory for the Rust daemon.

```toml
database = "oracle-candidate.redb"
sources = ["DIFFERENTIAL"]
listen = "127.0.0.1:9043"
max_connections = 4
io_timeout_seconds = 5
max_snapshot_bytes = 65536
```

Load the fixture and start the candidate in that terminal. Replace
`TEMP_DIR` with the path to the temporary directory.

```sh
cargo run --locked -- --config "TEMP_DIR/irrd.toml" import snapshot --source DIFFERENTIAL --file docs/compatibility/differential-whois.rpsl
cargo run --locked -- --config "TEMP_DIR/irrd.toml" server
```

In another terminal, run the [differential check](../../docs/benchmarks/METHODOLOGY.md#running-exact-key-whois-differential-checks).

```sh
python tools/differential_whois.py --reference 127.0.0.1:8043 --candidate 127.0.0.1:9043 --allow-filtered-auth-omission
```

The default corpus includes exact object keys (`!m`), exact IPv4 and IPv6
route searches with and without origins-only output (`!r`, `!r...,o`), and
IPv4 and IPv6 origin searches (`!g`, `!6`). Route misses include a prefix
covered by a fixture route, to check that exact matching does not return
the covering object. Formatting allowances apply only to object text.
Origins-only and prefix-list payloads are compared byte for byte.

If the oracle does not become healthy, inspect its project-specific logs.

```sh
docker compose -p irrd-454-oracle -f tools/oracle/compose.yaml logs
```

Stop the candidate with Ctrl+C. When the oracle is no longer needed, remove
its containers and test volume with the same dedicated project name.

```sh
docker compose -p irrd-454-oracle -f tools/oracle/compose.yaml down --volumes
```

On the initial Windows development host, `docker compose config --quiet`
passed and the `desktop-linux` Docker Engine timed out. The first Linux CI
run built and started the pinned upstream stack, loaded five fixture
objects into the Rust candidate and matched all 13 exact-key WHOIS
cases. The comparison checked framing before applying the documented
attribute-padding and filtered-auth placeholder allowances.
[Linux CI run](https://github.com/Peravix/Rirrd/actions/runs/41612205799)
