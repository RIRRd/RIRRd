#!/usr/bin/env bash
set -euo pipefail

die() {
    printf '%s\n' "$*" >&2
    exit 1
}

usage="usage: bash deploy/packages/build.sh [--arch amd64|arm64] BINARY OUTPUT_DIRECTORY"
package_machine=$(uname -m)
if [[ ${1-} == --arch ]]; then
    [[ $# -eq 4 ]] || die "$usage"
    case $2 in
        amd64) package_machine=x86_64 ;;
        arm64) package_machine=aarch64 ;;
        *) die "$usage" ;;
    esac
    shift 2
fi
[[ $# -eq 2 ]] || die "$usage"
[[ $(uname -s) = Linux ]] || die "package builds require Linux"
for tool in dpkg-deb readelf rpmbuild python3 cargo; do
    command -v "$tool" >/dev/null || die "required tool missing: $tool"
done
case $package_machine in
    x86_64) deb_arch=amd64; rpm_arch=x86_64; elf_arch='Advanced Micro Devices X86-64' ;;
    aarch64) deb_arch=arm64; rpm_arch=aarch64; elf_arch='AArch64' ;;
    *) die "package builds support only x86_64 and ARM64" ;;
esac

package_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
binary=$(realpath -- "$1")
[[ -x "$binary" ]] || die "expected an executable release binary"
if ! LC_ALL=C readelf -h "$binary" | grep -E "Machine:[[:space:]]+$elf_arch$" >/dev/null; then
    die "binary does not match the selected ELF architecture"
fi
rpm_cross_args=()
if [[ $package_machine != "$(uname -m)" ]]; then
    strip_tool="$package_machine-linux-gnu-strip"
    command -v "$strip_tool" >/dev/null || die "required cross strip tool missing: $strip_tool"
    rpm_cross_args+=(--define "__strip $(command -v "$strip_tool")")
fi
version=$("$binary" --version)
[[ $version =~ ^rust-irrd[[:space:]]([0-9]+\.[0-9]+\.[0-9]+)(-((alpha|beta|rc)\.[0-9]+))?$ ]] ||
    die "expected an x.y.z or x.y.z-beta.N rust-irrd binary"
base_version=${BASH_REMATCH[1]}
prerelease=${BASH_REMATCH[3]-}
version=$base_version
release=1
deb_version=$base_version
if [[ -n $prerelease ]]; then
    deb_version="$base_version~$prerelease"
    release="0.1.$prerelease"
fi
mkdir -p -- "$2"
output=$(realpath -- "$2")

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
# The artifact records its real ABI floor; a host-built beta must not advertise
# Debian 12 support when linked against a newer libc.
glibc_floor=$(LC_ALL=C readelf --version-info "$binary" | sed -nE 's/.*Name: GLIBC_([0-9]+\.[0-9]+).*/\1/p' | sort -V | tail -1)
[[ $glibc_floor =~ ^[0-9]+\.[0-9]+$ ]] || die "cannot determine binary glibc requirement"
if dpkg --compare-versions "$glibc_floor" lt 2.36; then glibc_floor=2.36; fi
license_target="$package_machine-unknown-linux-gnu"
python3 "$package_dir/../../tools/release/license-notices.py" --target "$license_target" --output "$work/THIRD_PARTY_LICENSES.txt"
sed 's|/usr/local/bin/rust-irrd|/usr/bin/rust-irrd|g' \
    "$package_dir/../rust-irrd.service" > "$work/rust-irrd.service"
sed 's/^listen = .*/listen = "127.0.0.1:4343"/' \
    "$package_dir/../irrd.toml" > "$work/irrd.toml"

stage="$work/deb"
install -d -m 0755 "$stage/DEBIAN"
install -D -m 0755 "$binary" "$stage/usr/bin/rust-irrd"
install -D -m 0644 "$work/rust-irrd.service" "$stage/usr/lib/systemd/system/rust-irrd.service"
install -d -m 0750 "$stage/etc/rust-irrd"
install -m 0640 "$work/irrd.toml" "$stage/etc/rust-irrd/irrd.toml"
install -D -m 0644 "$package_dir/rust-irrd.sysusers.conf" "$stage/usr/lib/sysusers.d/rust-irrd.conf"
install -D -m 0644 "$package_dir/rust-irrd.tmpfiles.conf" "$stage/usr/lib/tmpfiles.d/rust-irrd.conf"
install -D -m 0644 "$package_dir/copyright" "$stage/usr/share/doc/rust-irrd/copyright"
install -D -m 0644 "$package_dir/../../LICENSE" "$stage/usr/share/doc/rust-irrd/LICENSE"
install -D -m 0644 "$package_dir/../../licenses/ROUTINATOR-BSD-3-CLAUSE.txt" "$stage/usr/share/doc/rust-irrd/licenses/ROUTINATOR-BSD-3-CLAUSE.txt"
install -D -m 0644 "$package_dir/../../THIRD_PARTY_NOTICES.md" "$stage/usr/share/doc/rust-irrd/THIRD_PARTY_NOTICES.md"
install -D -m 0644 "$work/THIRD_PARTY_LICENSES.txt" "$stage/usr/share/doc/rust-irrd/THIRD_PARTY_LICENSES.txt"
for script in postinst prerm postrm; do
    install -m 0755 "$package_dir/debian/$script" "$stage/DEBIAN/$script"
done
printf '%s\n' /etc/rust-irrd/irrd.toml > "$stage/DEBIAN/conffiles"
cat > "$stage/DEBIAN/control" <<EOF
Package: rust-irrd
Version: $deb_version-1
Section: net
Priority: optional
Architecture: $deb_arch
Maintainer: Rirrd packaging <noreply@github.com>
Depends: libc6 (>= $glibc_floor), libgcc-s1, ca-certificates, systemd, init-system-helpers
Homepage: https://github.com/Peravix/Rirrd
Description: Rust Internet Routing Registry daemon
 The service requires an operator-provided snapshot and explicit startup.
EOF
dpkg-deb --root-owner-group --build "$stage" "$output/rust-irrd_${deb_version}-1_${deb_arch}.deb"

top="$work/rpm"
mkdir -p -- "$top/SOURCES"
install -m 0755 "$binary" "$top/SOURCES/rust-irrd"
install -m 0644 "$work/rust-irrd.service" "$work/irrd.toml" \
    "$package_dir/rust-irrd.sysusers.conf" "$package_dir/rust-irrd.tmpfiles.conf" \
    "$package_dir/copyright" "$package_dir/../../LICENSE" "$top/SOURCES/"
install -m 0644 "$package_dir/../../licenses/ROUTINATOR-BSD-3-CLAUSE.txt" "$package_dir/../../THIRD_PARTY_NOTICES.md" "$top/SOURCES/"
install -m 0644 "$work/THIRD_PARTY_LICENSES.txt" "$top/SOURCES/"
rpmbuild -bb --target "$rpm_arch" "${rpm_cross_args[@]}" \
    --define "_topdir $top" \
    --define "package_version $version" \
    --define "package_release $release" \
    "$package_dir/rust-irrd.spec"
install -m 0644 "$top/RPMS/$rpm_arch/rust-irrd-$version-$release.$rpm_arch.rpm" "$output/"
