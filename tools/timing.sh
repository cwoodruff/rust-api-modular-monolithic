#!/usr/bin/env bash
#
# Median wall-clock time per test suite, over N runs (default 3). Port of the
# original's tests/timing.sh.
#
#   tools/timing.sh [runs]
#
# Builds the tests once, then runs each suite N times with nothing else
# compiling, and prints the median and the median per test. The database
# suites start their own PostgreSQL container, so Docker must be running
# (or TEST_POSTGRES_URL set); the container start is included, as upstream's
# numbers include it.

set -euo pipefail

runs="${1:-3}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

suites=("architecture-tests" "module-tests" "api")

echo "building the tests..."
cargo test --workspace --no-run --quiet >/dev/null 2>&1

median() { sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }'; }

printf '%-24s %6s %12s %14s\n' Suite Tests "Median (s)" "Per test (ms)"

for suite in "${suites[@]}"; do
    times=()
    tests=0
    for _ in $(seq 1 "$runs"); do
        start=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
        output=$(cargo test -p "$suite" --quiet 2>&1) || { echo "$output" >&2; echo "$suite failed" >&2; exit 1; }
        end=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
        times+=("$(echo "$end - $start" | bc)")
        tests=$(echo "$output" | grep -Eo '[0-9]+ passed' | awk '{ s += $1 } END { print s + 0 }')
    done
    middle=$(printf '%s\n' "${times[@]}" | median)
    per_test=$(echo "scale=0; $middle * 1000 / ($tests + ($tests == 0))" | bc)
    printf '%-24s %6s %12.2f %14s\n' "$suite" "$tests" "$middle" "$per_test"
done
