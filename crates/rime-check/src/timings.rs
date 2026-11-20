//! Wall-clock accounting for `--timings` (CLI). Phases are recorded by name
//! and aggregated here; rendering is the CLI's job — this crate only counts.
//!
//! Costs nothing when absent: every instrumentation site is behind an
//! `Option<&mut Timings>` and skips the clock reads entirely on `None`.
//!
//! Two kinds of phase since the per-file passes went parallel: WALL phases
//! (serial sections and whole parallel blocks — these sum to the run) and
//! CPU phases (per-file/per-rule buckets summed across worker threads —
//! these show relative rule cost and can exceed the wall total). Workers
//! record into their own `Timings` and the driver folds them in with
//! `merge_cpu`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One named phase's total across the run.
pub struct Phase {
    pub name: &'static str,
    pub total: Duration,
    /// How many spans were folded in (files for per-file phases, bodies for
    /// per-body phases). Zero-cost phases keep their slot so the report's
    /// phase order is stable run to run.
    pub count: u64,
    /// Summed across worker threads (see the module doc) — not a share of
    /// the wall total.
    pub cpu: bool,
}

/// Aggregated wall-clock spans, in first-recorded order.
#[derive(Default)]
pub struct Timings {
    phases: Vec<Phase>,
    files: HashMap<PathBuf, Duration>,
}

impl Timings {
    /// Fold `dur` into the named wall phase, creating it on first sight.
    pub fn record(&mut self, name: &'static str, dur: Duration) {
        self.fold(name, dur, 1, false);
    }

    fn fold(&mut self, name: &'static str, dur: Duration, count: u64, cpu: bool) {
        match self.phases.iter_mut().find(|p| p.name == name && p.cpu == cpu) {
            Some(p) => {
                p.total += dur;
                p.count += count;
            }
            None => self.phases.push(Phase { name, total: dur, count, cpu }),
        }
    }

    /// Fold a worker thread's timings in: its phases become CPU phases
    /// here (summed, not wall), its per-file spans join the file table.
    pub fn merge_cpu(&mut self, other: Timings) {
        for p in other.phases {
            self.fold(p.name, p.total, p.count, true);
        }
        for (path, dur) in other.files {
            *self.files.entry(path).or_default() += dur;
        }
    }

    /// Time `f` under `name`.
    pub fn time<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(name, start.elapsed());
        out
    }

    /// Fold `dur` into one file's total (parse and check spans both land
    /// here; the report shows the sum).
    pub fn record_file(&mut self, path: &Path, dur: Duration) {
        *self.files.entry(path.to_path_buf()).or_default() += dur;
    }

    /// Phases in first-recorded order.
    pub fn phases(&self) -> &[Phase] {
        &self.phases
    }

    /// The `n` slowest files, slowest first.
    pub fn slowest_files(&self, n: usize) -> Vec<(&Path, Duration)> {
        let mut files: Vec<_> = self.files.iter().map(|(p, d)| (p.as_path(), *d)).collect();
        files.sort_by(|a, b| b.1.cmp(&a.1));
        files.truncate(n);
        files
    }
}

/// `timings.time(...)` through an `Option`, skipping the clock on `None`.
/// Either way `name` is also the crash-attribution phase (`guard::phase`):
/// a panic inside `f` is reported as happening in `name`.
pub fn time_opt<T>(
    timings: &mut Option<&mut Timings>,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    crate::guard::phase(name, || {
        crate::guard::fault("phase", Path::new(name));
        match timings {
            Some(t) => t.time(name, f),
            None => f(),
        }
    })
}
