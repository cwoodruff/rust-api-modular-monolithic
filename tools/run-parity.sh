#!/usr/bin/env bash
#
# Run the parity check: start the C# original and this port side by side,
# compare every route, then stop both.
#
#   tools/run-parity.sh /path/to/aspnetcore-min-api-modular-monolithic
#
# The path may also come from ORIGINAL_REPO. If neither is given, or the .NET
# SDK is missing, the script explains what it needs and exits 0 — a machine
# without the original is not a failure, it just cannot run this check.
#
# Each service gets its own copy of data/chinook.db. The write cases insert
# rows, and identical copies keep the generated keys lined up so the responses
# can be compared directly.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
original_repo="${1:-${ORIGINAL_REPO:-}}"

original_port="${ORIGINAL_PORT:-5043}"
port_port="${PORT_PORT:-5055}"

skip() {
    echo "skipping the parity check: $1"
    echo
    echo "It needs the original checked out and the .NET SDK installed:"
    echo "  git clone https://github.com/cwoodruff/aspnetcore-min-api-modular-monolithic"
    echo "  tools/run-parity.sh path/to/aspnetcore-min-api-modular-monolithic"
    exit 0
}

[ -n "$original_repo" ] || skip "no path to the original was given"
[ -d "$original_repo" ] || skip "$original_repo is not a directory"
command -v dotnet >/dev/null 2>&1 || skip "the dotnet CLI is not on PATH"

original_host="$original_repo/src/ModularMonolith.Api"
[ -d "$original_host" ] || skip "$original_host does not exist; is that the right repository?"

workspace="$(mktemp -d)"
original_pid=""
port_pid=""

cleanup() {
    [ -n "$original_pid" ] && kill "$original_pid" 2>/dev/null || true
    [ -n "$port_pid" ] && kill "$port_pid" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$workspace"
}
trap cleanup EXIT

# One database each, from the same source.
cp "$here/data/chinook.db" "$workspace/original.db"
cp "$here/data/chinook.db" "$workspace/port.db"

# The logins both services are seeded with. The original documents exactly
# this environment-variable form for seeding, and this port binds it too.
seed_logins() {
    export ASPNETCORE_ENVIRONMENT=Development

    export Identity__InMemoryUsers__0__Username=admin
    export Identity__InMemoryUsers__0__Password=secret123
    export Identity__InMemoryUsers__0__UserId=admin-1
    export Identity__InMemoryUsers__0__DisplayName=AdminUser
    export Identity__InMemoryUsers__0__Email=admin@example.com
    export Identity__InMemoryUsers__0__Tenant=tenant-1
    export Identity__InMemoryUsers__0__Roles__0=Admin
    export Identity__InMemoryUsers__0__Permissions__0=music.read
    export Identity__InMemoryUsers__0__Permissions__1=orders.read
    export Identity__InMemoryUsers__0__Permissions__2=administration.read
    export Identity__InMemoryUsers__0__Permissions__3=administration.write

    # Holds the read permissions but not administration.write.
    export Identity__InMemoryUsers__1__Username=reader
    export Identity__InMemoryUsers__1__Password=secret123
    export Identity__InMemoryUsers__1__UserId=user-1
    export Identity__InMemoryUsers__1__Tenant=tenant-1
    export Identity__InMemoryUsers__1__Roles__0=Admin
    export Identity__InMemoryUsers__1__Permissions__0=administration.read

    # Holds the administration permissions but not the Admin role.
    export Identity__InMemoryUsers__2__Username=roleless
    export Identity__InMemoryUsers__2__Password=secret123
    export Identity__InMemoryUsers__2__UserId=user-2
    export Identity__InMemoryUsers__2__Tenant=tenant-1
    export Identity__InMemoryUsers__2__Roles__0=User
    export Identity__InMemoryUsers__2__Permissions__0=administration.read
    export Identity__InMemoryUsers__2__Permissions__1=administration.write

    # Holds everything except a tenant claim.
    export Identity__InMemoryUsers__3__Username=tenantless
    export Identity__InMemoryUsers__3__Password=secret123
    export Identity__InMemoryUsers__3__UserId=user-3
    export Identity__InMemoryUsers__3__Roles__0=Admin
    export Identity__InMemoryUsers__3__Permissions__0=administration.read
    export Identity__InMemoryUsers__3__Permissions__1=administration.write
}

wait_for() {
    local url="$1" name="$2"
    for _ in $(seq 1 60); do
        if curl -sS -o /dev/null "$url" 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "$name did not come up at $url" >&2
    return 1
}

echo "building the original..."
(cd "$original_repo" && dotnet build ModularMonolith.Api.sln -v q --nologo >"$workspace/build-original.log" 2>&1) \
    || { echo "the original failed to build; see $workspace/build-original.log" >&2; cat "$workspace/build-original.log" >&2; exit 1; }

echo "building the port..."
(cd "$here" && cargo build --quiet -p api)

echo "starting the original on :$original_port"
(
    cd "$original_host"
    seed_logins
    export ConnectionStrings__AppDatabase="Data Source=$workspace/original.db"
    export ASPNETCORE_URLS="http://localhost:$original_port"
    exec dotnet run --no-build -v q >"$workspace/original.log" 2>&1
) &
original_pid=$!

echo "starting the port on :$port_port"
(
    cd "$here"
    seed_logins
    export ConnectionStrings__AppDatabase="Data Source=$workspace/port.db"
    export Port="$port_port"
    exec ./target/debug/modular-monolith-api >"$workspace/port.log" 2>&1
) &
port_pid=$!

wait_for "http://localhost:$original_port/" "the original" || { cat "$workspace/original.log" >&2; exit 1; }
wait_for "http://localhost:$port_port/" "the port" || { cat "$workspace/port.log" >&2; exit 1; }

echo
python3 "$here/tools/parity-diff.py" \
    --original "http://localhost:$original_port" \
    --port "http://localhost:$port_port" \
    "${@:2}"
