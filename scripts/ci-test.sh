#!/usr/bin/env bash
#
# Runs the whole test suite and passes only when exactly the corpus cases
# listed in tests/expected-red.txt fail.
#
# `cargo test --workspace` exits non-zero on purpose: those cases specify
# rules that are not built yet. This script makes two runs, and between them
# every test runs as written:
#
#   1. Everything except the listed cases. Any failure fails the script.
#   2. The whole corpus target. The cases that fail must be the listed ones,
#      no more and no fewer, so a listed case that starts to pass fails the
#      script until its line is deleted.
#
# Usage: scripts/ci-test.sh   (bash 3.2 or later; Git Bash on Windows)

set -euo pipefail

cd "$(dirname "$0")/.."

list=tests/expected-red.txt
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

report() {
    if [ "${GITHUB_ACTIONS:-}" = true ]; then
        echo "::error::$*"
    else
        echo "ci-test: error: $*" >&2
    fi
}
fail() {
    report "$@"
    exit 1
}
# Problems found in run 2 are held back and reported under the corpus output.
: > "$work/problems"
problem() {
    echo "$*" >> "$work/problems"
}

# --- the list -------------------------------------------------------------

[ -f "$list" ] || fail "$list is missing"

tr -d '\r' < "$list" \
    | sed -e 's/#.*$//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' \
    | { grep -v '^$' || true; } \
    | LC_ALL=C sort > "$work/expected"

duplicates="$(uniq -d "$work/expected")"
[ -z "$duplicates" ] || fail "$list names a case twice: $duplicates"

skips=()
while IFS= read -r name; do
    case "$name" in
        *\\*) fail "$list: write '$name' with forward slashes" ;;
    esac
    [ -e "tests/$name" ] || fail "$list names tests/$name, which does not exist"
    # The harness names a case by its path under tests/, with the separator
    # of the platform it runs on, so skip both spellings. With --exact a skip
    # matches that one name and nothing else.
    skips+=(--skip "$name" --skip "$(printf '%s' "$name" | tr '/' '\\')")
done < "$work/expected"
expected_count="$(wc -l < "$work/expected" | tr -d '[:space:]')"

# --- run 1: everything else must pass ---------------------------------------

echo "ci-test: running every test except the $expected_count cases in $list"
cargo test --workspace --locked --no-fail-fast -- --exact ${skips[@]+"${skips[@]}"} \
    || fail "a test that is not in $list failed, or the tests did not build; see the output above"

# --- run 2: the corpus must fail in exactly the listed cases ---------------

echo "ci-test: running the corpus; exactly the $expected_count listed cases must fail"
set +e
# The harness prints its `test ... ok` lines and its summary on stdout. Keep
# stderr apart: a line a test thread or cargo writes there could land in the
# middle of a line that is parsed below.
cargo test -p rime-check --test corpus --locked --no-fail-fast -- --color never \
    > "$work/corpus.log" 2> "$work/corpus.err"
status=$?
set -e

# Prints the names of the cases with the given outcome, one per line, with
# forward slashes.
cases_with_outcome() {
    tr -d '\r' < "$work/corpus.log" \
        | sed -n "s/^test \(.*[^ ]\) *\.\.\. $1\$/\1/p" \
        | tr '\\' '/' \
        | LC_ALL=C sort
}
cases_with_outcome FAILED > "$work/failed"
cases_with_outcome ok > "$work/passed"
failed_count="$(wc -l < "$work/failed" | tr -d '[:space:]')"

while IFS= read -r name; do
    problem "$name failed and is not in $list"
done < <(LC_ALL=C comm -13 "$work/expected" "$work/failed")

while IFS= read -r name; do
    if grep -Fxq -- "$name" "$work/passed"; then
        problem "$name passes now: delete its line from $list"
    else
        problem "$name is in $list but the corpus harness did not run it"
    fi
done < <(LC_ALL=C comm -23 "$work/expected" "$work/failed")

# The parsed lines must agree with what the harness itself reported, so a
# crash or a change in its output format cannot pass as "nothing failed".
if ! grep -Eq "^test result: .* passed; $failed_count failed;" "$work/corpus.log"; then
    problem "the corpus summary does not report $failed_count failed cases"
fi
if [ "$failed_count" -eq 0 ] && [ "$status" -ne 0 ]; then
    problem "the corpus target exited $status without a failed case"
fi
if [ "$failed_count" -ne 0 ] && [ "$status" -eq 0 ]; then
    problem "the corpus target exited 0 with $failed_count failed cases"
fi

if [ -s "$work/problems" ]; then
    cat "$work/corpus.err" "$work/corpus.log"
    while IFS= read -r line; do
        report "$line"
    done < "$work/problems"
    exit 1
fi

echo "ci-test: ok. Every other test passed, and the corpus failed in exactly the $expected_count listed cases."
