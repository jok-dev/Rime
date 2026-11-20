#!/usr/bin/env bash
# Fetch a benchmark / fidelity corpus.
#
# Wants real code, not generated input: deeply nested tables and long call
# chains behave differently from a repeated 1k-line block.
#
# Three sources, in increasing order of how representative they are:
#   1. Luau's own conformance + test suite  — exercises exotic syntax on purpose
#   2. open-source Roblox libraries         — idiomatic community code
#   3. your own project                     — the only truly representative one
#
# The third is the one that should drive the decision. Point
# RIME_BENCH_CORPUS at your own code if you have some.

set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p corpus
cd corpus

clone() {
  local url="$1" dir="$2"
  if [ -d "$dir" ]; then
    echo "  have $dir"
  else
    echo "  cloning $dir"
    git clone --depth 1 --quiet "$url" "$dir"
  fi
}

echo "tier 1: luau upstream"
clone https://github.com/luau-lang/luau.git luau

echo "tier 2: community libraries (edit scripts/corpus-sources.txt to add)"
if [ -f ../scripts/corpus-sources.txt ]; then
  while read -r url; do
    [ -z "$url" ] && continue
    case "$url" in \#*) continue ;; esac
    name="$(basename "$url" .git)"
    clone "$url" "$name" || echo "  SKIP $url (verify the repo exists)"
  done < ../scripts/corpus-sources.txt
fi

echo
echo "corpus summary:"
find . -name '*.luau' -o -name '*.lua' | wc -l | xargs echo "  files:"
find . \( -name '*.luau' -o -name '*.lua' \) -exec cat {} + | wc -l | xargs echo "  lines:"
echo
echo "next:"
echo "  cargo run --release --bin rime-fidelity     # the decision-relevant one"
echo "  cargo run --release --bin rime-rss"
echo "  cargo bench -p rime-bench"
