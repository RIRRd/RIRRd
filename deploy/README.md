# Deployment examples

The server requires an existing registry database. Import a snapshot as the service user before the first start. Stop the server before running import, export, verification, purge or NRTM maintenance against the same redb file. Keep the state directory and raw snapshots private because stored objects may contain maintainer credentials.

## Docker Compose

Run these commands from the repository root in a POSIX shell. The image runs as UID and GID `65532`. Its state lives on a named volume, and the supplied service publishes WHOIS on `127.0.0.1:4343` on the host. GraphQL is disabled in the example configuration.

```sh
docker compose -f deploy/compose.yaml config --quiet
docker compose -f deploy/compose.yaml build --pull
docker compose -f deploy/compose.yaml run --rm --no-deps \
  --volume "$PWD/examples/example.rpsl:/snapshot.rpsl:ro" \
  irrd import snapshot --source EXAMPLE --file /snapshot.rpsl
docker compose -f deploy/compose.yaml run --rm --no-deps irrd database verify
docker compose -f deploy/compose.yaml up -d
printf '!mroute,192.0.2.0/24AS64496\n' | nc 127.0.0.1 4343
```

For real data, set the ordered source names in `deploy/irrd.toml` and mount each corresponding snapshot for its one-off import. Bind-mounted inputs and configuration must be readable by UID `65532` inside the container. Use Docker Engine 28 or later for host-loopback publishing. [Docker port publishing](https://docs.docker.com/engine/network/port-publishing/)

`docker compose -f deploy/compose.yaml stop` sends SIGTERM and allows up to 30 seconds for shutdown. The named volume persists across container replacement. The health check sends WHOIS `!v` and validates the reply framing. Use the offline `database verify` command to check index consistency.

The build uses the official Rust `1.99.1-bookworm` and Debian `12.15-slim` tags. Build with the checked-in Cargo lockfile. [Rust image](https://hub.docker.com/_/rust) · [Debian image](https://hub.docker.com/_/debian)

## Native Linux with systemd

Run from the repository root on a Linux host with Rust and the native linker. The example runs WHOIS on loopback port 4343. Create the service account once.

```sh
cargo build --locked --release
sudo install -m 0755 target/release/rust-irrd /usr/local/bin/rust-irrd
sudo useradd --system --user-group --no-create-home \
  --home-dir /var/lib/rust-irrd --shell /usr/sbin/nologin rust-irrd
sudo install -d -m 0700 -o rust-irrd -g rust-irrd \
  /var/lib/rust-irrd /var/lib/rust-irrd/state
sudo install -d -m 0750 -o root -g rust-irrd /etc/rust-irrd
sudo install -m 0640 -o root -g rust-irrd deploy/irrd.toml /etc/rust-irrd/irrd.toml
sudo sed -i 's/^listen = .*/listen = "127.0.0.1:4343"/' /etc/rust-irrd/irrd.toml
sudo install -m 0600 -o rust-irrd -g rust-irrd \
  examples/example.rpsl /var/lib/rust-irrd/bootstrap.rpsl
sudo -u rust-irrd sh -c 'umask 077
  exec /usr/local/bin/rust-irrd --config /etc/rust-irrd/irrd.toml \
  import snapshot --source EXAMPLE --file /var/lib/rust-irrd/bootstrap.rpsl'
sudo -u rust-irrd /usr/local/bin/rust-irrd --config /etc/rust-irrd/irrd.toml database verify
sudo install -m 0644 deploy/rust-irrd.service /etc/systemd/system/rust-irrd.service
sudo systemctl daemon-reload
sudo systemctl enable --now rust-irrd.service
```

Use `journalctl -u rust-irrd.service` to inspect service logs. If you enable GraphQL by adding `http_listen` to the native configuration, the daemon requires a loopback address. Keep the database under `/var/lib/rust-irrd` for write access within the unit's filesystem restrictions.

## Validation status

`docker compose -f deploy/compose.yaml config --quiet` and the daemon's `config check` passed on the Windows development host. The configured Docker Desktop Linux engine pipe was unavailable. Image build, container startup, volume permissions, health checks, signal handling and native systemd startup need runtime validation on a Linux or Docker-enabled host.
