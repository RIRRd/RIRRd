#!/usr/bin/env bash
# Download a tagged beta package and verify its published SHA-256 checksum.
set -euo pipefail
version=0.1.0-beta.3
format=auto
destination=
usage() { printf '%s\n' 'Usage: bash tools/install.sh [--version X.Y.Z-beta.N] [--format deb|rpm] [--download-only DIRECTORY]'; }
die() { printf '%s\n' "$*" >&2; exit 1; }
while (($#)); do
    case $1 in
        --version|--format|--download-only)
            (($# >= 2)) || die 'Missing option value'
            case $1 in --version) version=$2;; --format) format=$2;; --download-only) destination=$2;; esac
            shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; die 'Unknown argument' ;;
    esac
done
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$ ]] || die 'Select an explicit beta version'
[[ $(uname -s) = Linux ]] || die 'Release packages require Linux'
case $(uname -m) in
    x86_64) deb_arch=amd64; rpm_arch=x86_64;;
    aarch64|arm64) deb_arch=arm64; rpm_arch=aarch64;;
    *) die 'Supported package architectures: x86-64 and ARM64';;
esac
if [[ $format = auto ]]; then
    if command -v apt-get >/dev/null; then format=deb
    elif command -v dnf >/dev/null; then format=rpm
    else die 'Select --format deb or --format rpm for downloading'; fi
fi
case $format in
    deb) archive="rust-irrd_${version%%-*}.${version#*-}-1_${deb_arch}.deb"; manager=apt-get;;
    rpm) archive="rust-irrd-${version%%-*}-0.1.${version#*-}.${rpm_arch}.rpm"; manager=dnf;;
    *) die 'Package format must be deb or rpm';;
esac
for tool in curl sha256sum awk mktemp; do command -v "$tool" >/dev/null || die "Missing required tool: $tool"; done
if [[ -z $destination ]]; then
    [[ $EUID -eq 0 ]] || die 'Install as root, or use --download-only DIRECTORY'
    command -v "$manager" >/dev/null || die "Missing package manager: $manager"
fi
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
release="https://github.com/RIRRd/RIRRd/releases/download/v$version"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 300 --output "$work/$archive" "$release/$archive"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 60 --output "$work/SHA256SUMS" "$release/SHA256SUMS"
# Match exactly one expected basename. Never trust a manifest's arbitrary paths.
awk -v name="$archive" '$2 == "./" name || $2 == name { print $1 "  " name; count++ } END { if (count != 1) exit 1 }' "$work/SHA256SUMS" > "$work/selected.sha256" || die 'Checksum manifest does not contain exactly one matching package'
(cd "$work" && sha256sum --strict --check selected.sha256) || die 'Package checksum verification failed'
if [[ -n $destination ]]; then
    mkdir -p -- "$destination"
    [[ ! -e $destination/$archive && ! -e $destination/SHA256SUMS ]] || die 'Download destination already contains release files'
    install -m 0644 "$work/$archive" "$work/SHA256SUMS" "$destination/"
    printf 'Verified package saved to %s/%s\n' "$destination" "$archive"
else
    case $format in
        deb) apt-get install -- "$work/$archive";;
        rpm) dnf install -- "$work/$archive";;
    esac
    printf '%s\n' 'Configure /etc/rust-irrd/irrd.toml and import your snapshot before explicitly starting rust-irrd.service.'
fi
