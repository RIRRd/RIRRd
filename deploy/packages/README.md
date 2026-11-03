# Native Linux packages

These definitions build a Debian package and an RPM for an `x86_64` or
`aarch64` release binary. The default target is the packaging host architecture. Use a Debian 12 build environment
for the same glibc baseline as CI. The Linux packaging host needs Bash,
`dpkg-deb`, `readelf` and `rpmbuild`.

```sh
cargo build --locked --release
bash deploy/packages/build.sh target/release/rust-irrd dist
```

The package builder checks the ELF architecture and uses the binary's
`x.y.z` version. The workflow builds on both native architectures, checks
package metadata and files, and installs in disposable Debian 12 and
Fedora 43 containers. It retains archives and SHA-256 checksums as short-lived
workflow artifacts. The containers verify the systemd unit, configuration and
state ownership, import a fixture as the service account, query the installed
binary and verify the database after SIGTERM. The RPM uses this direct-process
test in Fedora 43.

The workflow also installs the Debian archive on native Ubuntu 24.04 amd64
and ARM64 runners. On each host, it imports a fixture as the service account,
starts the packaged unit through systemd, queries the running daemon over
WHOIS, stops it through systemd and verifies the database. Both supervised
checks passed in the linked run.
[Workflow](../../.github/workflows/native-packages.yml) · [Passing run](https://github.com/Peravix/Rirrd/actions/runs/41615975939)

## Explicit architecture and cross builds

`--arch amd64` and `--arch arm64` select package metadata and the required ELF
architecture explicitly. An ELF mismatch fails before any package is created.
Cross packaging needs the matching GNU cross `strip` tool and a way to run the
binary's `--version` command, such as QEMU with binfmt registration. It does not
convert a host executable into another architecture.

For an ARM64 development check on an x86-64 Debian host:

```sh
sudo apt install gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user qemu-user-binfmt rpm
rustup target add aarch64-unknown-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64 -L /usr/aarch64-linux-gnu'
export QEMU_LD_PREFIX=/usr/aarch64-linux-gnu
cargo test --locked --release --target aarch64-unknown-linux-gnu --all-targets
cargo build --locked --release --target aarch64-unknown-linux-gnu
bash deploy/packages/build.sh --arch arm64 target/aarch64-unknown-linux-gnu/release/rust-irrd dist-arm64
```

Use the pinned baseline build environment and native CI for release qualification.
Emulated checks establish execution under QEMU, not native ARM64 performance.
The [upgraded VM evidence](../../docs/compatibility/UPGRADED_VM_CHECKPOINT.md)
records both successful release-profile checks and an emulated debug timeout.

## Install and bootstrap

Install the single package matching the target architecture. On Debian, run
`sudo apt install ./rust-irrd_VERSION_ARCH.deb`. On Fedora, run
`sudo dnf install ./rust-irrd-VERSION.ARCH.rpm`. Replace the filenames with
the artifacts from the build.

Installation creates the `rust-irrd` system account and private state paths
under `/var/lib/rust-irrd`. The default configuration is
`/etc/rust-irrd/irrd.toml`, readable by the daemon's group, and binds WHOIS
to `127.0.0.1:4343`. GraphQL and HTTP submissions require explicit
configuration. Package installation and upgrade leave startup and restart
decisions to the operator. Existing operator configuration is preserved by
the package manager. The state remains after package removal.

On a new install, edit the config to list the sources to import and make a
snapshot available to the service account. The following commands use the
repository's example source. Run them from the repository root.

```sh
sudo install -m 0600 -o rust-irrd -g rust-irrd \
  examples/example.rpsl /var/lib/rust-irrd/bootstrap.rpsl
sudo -u rust-irrd sh -c 'umask 077
  exec /usr/bin/rust-irrd --config /etc/rust-irrd/irrd.toml \
    import snapshot --source EXAMPLE --file /var/lib/rust-irrd/bootstrap.rpsl'
sudo -u rust-irrd /usr/bin/rust-irrd --config /etc/rust-irrd/irrd.toml database verify
sudo systemctl enable --now rust-irrd.service
```

## Licensing

Project licensing is pending. The RPM uses `LicenseRef-Proprietary` and the
Debian archive carries the package copyright notice. The workflow does not
sign the archives or publish a release. Third-party dependency terms apply.

## Packaging references

- Debian binary package archives and maintainer scripts. [dpkg-deb](https://manpages.debian.org/bookworm/dpkg/dpkg-deb.1.en.html) · [Debian Policy](https://www.debian.org/doc/debian-policy/ch-maintainerscripts.html)
- RPM metadata and binary packaging. [RPM spec](https://rpm.org/docs/latest/manual/spec.html)
- Declarative system account and filesystem permissions. [sysusers.d](https://manpages.debian.org/bookworm/systemd/sysusers.d.5.en.html) · [tmpfiles.d](https://www.freedesktop.org/software/systemd/man/latest/tmpfiles.d.html)
