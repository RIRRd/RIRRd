#!/usr/bin/env bash
# Run only in a disposable package test container or runner.
set -euo pipefail
export LC_ALL=C

supervised=0
if [[ ${1-} == --systemd ]]; then
    supervised=1
    shift
fi
[[ $# -eq 1 ]]
if (( supervised )); then
    [[ -d /run/systemd/system ]]
else
    [[ ! -d /run/systemd/system ]]
fi
fixture=$1
config=/etc/rust-irrd/irrd.toml
state=/var/lib/rust-irrd/state
binary=/usr/bin/rust-irrd
pid_file=$state/package-smoke.pid
launcher=
started=0

cleanup() {
    if (( started )); then
        systemctl stop rust-irrd.service || true
    fi
    if [[ -n $launcher ]]; then
        if [[ -s $pid_file ]]; then
            kill -TERM "$(cat "$pid_file")" 2>/dev/null || true
        fi
        wait "$launcher" 2>/dev/null || true
    fi
}
trap cleanup EXIT

test "$(stat -c '%U:%G:%a' /etc/rust-irrd)" = root:rust-irrd:750
test "$(stat -c '%U:%G:%a' "$config")" = root:rust-irrd:640
test "$(stat -c '%U:%G:%a' /var/lib/rust-irrd)" = rust-irrd:rust-irrd:700
test "$(stat -c '%U:%G:%a' "$state")" = rust-irrd:rust-irrd:700
test "$(systemctl --root=/ is-enabled rust-irrd.service 2>/dev/null || true)" = disabled
test ! -e "$state/irrd.redb"
systemd-analyze verify /usr/lib/systemd/system/rust-irrd.service
runuser -u rust-irrd -- "$binary" --config "$config" config check
runuser -u rust-irrd -- sh -c 'umask 077
    exec "$1" --config "$2" import snapshot --source EXAMPLE --file "$3"
' sh "$binary" "$config" "$fixture"
test "$(stat -c '%U:%G:%a' "$state/irrd.redb")" = rust-irrd:rust-irrd:600

if (( supervised )); then
    systemctl start rust-irrd.service
    started=1
else
    runuser -u rust-irrd -- sh -c 'umask 077
        echo "$$" > "$3"
        exec "$1" --config "$2" server
    ' sh "$binary" "$config" "$pid_file" &
    launcher=$!
fi
for attempt in {1..50}; do
    if { exec 3<>/dev/tcp/127.0.0.1/4343; } 2>/dev/null; then
        break
    fi
    if (( supervised )); then
        systemctl is-active --quiet rust-irrd.service
    else
        kill -0 "$launcher"
    fi
    sleep 0.1
done
printf '!mroute,192.0.2.0/24AS64496\r\n' >&3
IFS= read -r -t 5 header <&3
[[ $header =~ ^A([1-9][0-9]{0,5})$ ]]
IFS= read -r -t 5 -N "${BASH_REMATCH[1]}" object <&3
[[ $object == *'192.0.2.0/24'* ]]
IFS= read -r -t 5 trailer <&3
[[ $trailer = C ]]
exec 3>&-
if (( supervised )); then
    systemctl stop rust-irrd.service
    started=0
else
    kill -TERM "$(cat "$pid_file")"
    wait "$launcher"
    launcher=
fi
runuser -u rust-irrd -- "$binary" --config "$config" database verify
