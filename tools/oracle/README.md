# IRRd 4.5.4 WHOIS test oracle

This Docker Compose stack starts the pinned upstream daemon with PostgreSQL,
Redis and the repository's synthetic RPSL fixture. Both WHOIS transports
listen on host loopback only. The
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
http_listen = "127.0.0.1:9080"
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
python tools/differential_whois.py --reference 127.0.0.1:8043 --candidate 127.0.0.1:9043 --allow-filtered-auth-omission --normalize-attribute-padding
python tools/differential_whois.py --http --reference 127.0.0.1:8000 --candidate 127.0.0.1:9080 --allow-filtered-auth-omission --normalize-attribute-padding
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
[Initial Linux run](https://github.com/Peravix/Rirrd/actions/runs/41612205799)

The expanded fixture matched all 25 TCP cases and all 25 HTTP cases in Linux
CI. The workflow waits for both listeners and reloads the pinned daemon's
preload cache after they have started. [Expanded Linux run](https://github.com/Peravix/Rirrd/actions/runs/41616928804)

## Organisation schema and comparison smoke

The expanded candidate corpus adds a synthetic organisation and dual-stack
nearest/all-covering and more-specific boundary queries. After bringing up the
pinned reference, check the organisation schema independently:

```sh
docker compose -p irrd-454-oracle -f tools/oracle/compose.yaml exec -T reference python < tools/oracle/check_organisation_schema.py
```

The schema cases match the Rust validator's required fields, single-valued
organisation/name, repeatable contact details and rejection of invalid fields.
They do not establish upstream submission authorization parity.

For a small same-host comparative timing check after correctness passes:

```sh
python3 tools/oracle/benchmark_pair.py --reference 127.0.0.1:8043 --candidate 127.0.0.1:9043 --candidate-commit "$(git rev-parse HEAD)"
```

Run without concurrent builds or fuzzing; record image ID, `pip freeze`, process
memory and persistent storage separately. This six-object smoke includes a new
TCP connection per request and is not sustained-load or release evidence. Use
`docs/benchmarks/METHODOLOGY.md` for qualification.

With both HTTP listeners ready, `python3 tools/oracle/compare_organisation_graphql.py`
compares organisation values and nine supported field types against pinned upstream.
It intentionally does not treat missing relationship/journal/updated fields as parity.
The WHOIS corpus now contains 39 cases per transport.

## Authoritative policy-set oracle

The runtime configuration adds two synthetic authoritative sources, AUTH-TEST and
AUTH-OTHER. They are separate from the read-only DIFFERENTIAL query fixture.
`make_auth_fixture.py` generates test-only bcrypt credentials in the pinned Python
container. Load its output once into both implementations (with operator restore
for the Rust candidate), then run `compare_policy_set_auth.py`. The CI workflow
contains the exact bootstrap sequence. The comparator checks 30 normal HTTP
submission outcomes: wrong/owner/parent proofs, create/modify/delete, missing
contacts and cross-source denial, with secret-free reports. It does not compare
notification delivery or establish account/API-token/OpenPGP parity.

`check_policy_set_schema.py` checks 17 strict parser decisions mirrored by the
Rust validator tests, including the reference's empty comma-token handling.
The reference runtime starts `smtp_sink.py` on container loopback port 1025.
It discards synthetic notifications and never relays or logs message content.
No SMTP port is published. Generated credentials/fixtures stay outside Git.

The authoritative comparison now includes `inet-rtr` and existing-maintainer
`BCRYPT-PW DummyValue` replacement: 47 HTTP decision cases in total. The schema
check covers 25 policy-set/router cases. Dummy replacement requires one distinct
password after authentication against stored maintainers. Duplicate identical
passwords are normalised; distinct multiple passwords and mixed dummy/real
hashes are rejected. No dummy hash is stored, and replacement hashing is charged
to the submission work budget. Maintainer creation still requires provisioning.

For repeated timing samples, use `benchmark_pair.py --first reference` and
`--first candidate` in alternating runs. Set `--reference-workers` to the actual
reference configuration and pass `--candidate-binary` to record its SHA-256.
The helper records timestamps, fixture SHA-256 and host resources. Changing the
worker configuration requires a separate IRRd runtime config and restart; the
argument records metadata and does not reconfigure the daemon. Retain each run.
The 24-CPU VM checkpoint uses five runs, eight IRRd workers and a Rust connection
cap of eight; the historical single-worker reference results are a different profile.

## AS-block and domain continuation

`check_delegation_schema.py` covers 16 AS-block/domain schema cases. The normal
HTTP comparator now checks 71 authorization outcomes, including distinct domain
zone-contact references, maintainer denial, missing/cross-source contacts and
delete protection. The WHOIS corpus has 45 TCP and 45 HTTP cases using the
12-object original synthetic fixture. These are bounded profiles; domain's
upstream parser makes mnt-by optional, but authenticated writes here require it.
Opaque domain delegation text is preserved without DNS resolution or DNS changes.
`check_mirror_admission.py` checks three non-strict upstream rejections of unsafe
control characters inside AS-set member names; the deployment withholds the
complete record rather than inventing a cleaned member identity.

For repeatable authorization runs, use a fresh isolated PostgreSQL volume. Loading
an RPSL source clears its objects and journal, but upstream protected-name tombstones
can survive that load. A repeated suite can therefore reject a synthetic contact
name that a previous test deleted. Reset only the dedicated oracle volume (never
an operational registry database), or create a new named test volume. The normal
CI workflow already creates and removes its isolated volume per run.
