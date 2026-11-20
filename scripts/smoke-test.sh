#!/usr/bin/env bash
#
# Runs a built `rime` executable far enough to know that it starts and checks
# files on this machine. The release workflow runs it on every executable
# before publishing.
#
# Usage: scripts/smoke-test.sh <executable> <expected version>

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <executable> <expected version>" >&2
    exit 2
fi
version="$2"

fail() {
    if [ "${GITHUB_ACTIONS:-}" = true ]; then
        echo "::error::$*"
    else
        echo "smoke-test: error: $*" >&2
    fi
    exit 1
}

[ -f "$1" ] || fail "$1 is not a file"
bin="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
cd "$(dirname "$0")/.."

reported="$("$bin" --version)" || fail "$1 --version did not run"
echo "$reported"
[ "$reported" = "rime $version" ] || fail "$1 reports '$reported', expected 'rime $version'"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# A clean file must check clean.
printf 'local greeting = "hello"\nprint(greeting)\n' > "$scratch/clean.luau"
"$bin" check "$scratch/clean.luau" --plain || fail "$1 did not accept a clean file"

# The test corpus has cases that must be rejected, so checking it ends with
# exit code 1. Any other code is a crash. The reports go to stdout and are
# dropped; the summary line goes to stderr.
status=0
"$bin" check tests --plain > /dev/null || status=$?
[ "$status" -eq 1 ] || fail "$1 exited $status on the test corpus, expected 1"

echo "smoke-test: ok"
