//! Parse benchmarks: full-moon vs Luau's own parser.
//!
//! WHAT THIS IS ACTUALLY FOR
//!
//! The parse is almost certainly not the bottleneck — dataflow and type
//! checking will dominate. Raw throughput is therefore NOT the decision
//! variable. The two numbers that can actually disqualify full-moon are:
//!
//!   1. single-file p99 latency  — a 5x parse slowdown is irrelevant in CI
//!                                 and fatal inside a 16ms LSP keystroke budget
//!   2. peak RSS at repo scale   — full-moon retains a lossless CST with trivia
//!                                 attached; Luau uses a bump-allocated lossy
//!                                 AST. This is the likelier disqualifier.
//!
//! Thresholds, set BEFORE running so the result decides something:
//!
//!   single-file p99   > 10ms   -> reconsider
//!   full-repo RSS     > 1GB    -> reconsider
//!   fidelity gap      > 0      -> see `fidelity` bin; this outweighs both
//!
//! METHODOLOGY NOTES (parser benchmarks are easy to get wrong)
//!   - all files are read into memory up front; we are not benchmarking disk
//!   - discard the first run; the OS cache must be warm
//!   - like-for-like is imperfect: full-moon builds a lossless CST with
//!     trivia, `Luau::parse` builds a lossy AST. Trivia is NOT optional for us
//!     (`--!rime` pragmas, future formatter-grade LSP), so the comparison is
//!     "what we need" vs "what they build", which is the honest framing.
//!   - mlua's `load` is parse + COMPILE to bytecode, so it overstates Luau's
//!     parse time. Treat the mlua number as an upper bound; for a true
//!     parse-only comparison, link `Luau.Ast` directly via a cc shim.
//!   - real code only. Generated input with a repeated 1k-line block does not
//!     exercise deep table nesting or long call chains.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::path::PathBuf;

fn corpus() -> Vec<(String, String)> {
    let dir = std::env::var("RIME_BENCH_CORPUS").unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .to_string_lossy()
            .into_owned()
    });

    let mut files: Vec<(String, String)> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "luau" || x == "lua")
        })
        .filter_map(|e| {
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((e.path().to_string_lossy().into_owned(), src))
        })
        .collect();

    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        panic!(
            "no corpus files under {dir} — run scripts/fetch-corpus.sh first, \
             or set RIME_BENCH_CORPUS"
        );
    }
    files
}

/// Per-file parse latency. This is the LSP-relevant number.
///
/// Report the DISTRIBUTION, not the total: one pathological file matters for
/// keystroke latency and vanishes in an aggregate.
fn bench_single_file(c: &mut Criterion) {
    let files = corpus();

    // Representative sample: smallest, median, largest by byte length.
    let mut by_len = files.clone();
    by_len.sort_by_key(|(_, s)| s.len());
    let picks = [
        ("small", &by_len[0]),
        ("median", &by_len[by_len.len() / 2]),
        ("large", &by_len[by_len.len() - 1]),
    ];

    let mut group = c.benchmark_group("parse/single-file");
    for (label, (path, src)) in picks {
        group.throughput(Throughput::Bytes(src.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("full_moon", format!("{label} ({} B)", src.len())),
            src,
            |b, src| b.iter(|| std::hint::black_box(full_moon::parse(src))),
        );
        let _ = path;
    }
    group.finish();
}

/// Whole-repo cold parse. This is the CI-relevant number.
fn bench_full_repo(c: &mut Criterion) {
    let files = corpus();
    let total: usize = files.iter().map(|(_, s)| s.len()).sum();

    let mut group = c.benchmark_group("parse/full-repo");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(total as u64));
    group.bench_function("full_moon", |b| {
        b.iter(|| {
            for (_, src) in &files {
                std::hint::black_box(full_moon::parse(src)).ok();
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_single_file, bench_full_repo);
criterion_main!(benches);
