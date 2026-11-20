//! Crash containment: a panic while working on one file (or one
//! whole-project rule) becomes an R0901 `internal-error` diagnostic against
//! that file instead of ending the run.
//!
//! Without this, one `unwrap` on one odd file takes the whole `rime check`
//! down — rayon carries a worker's panic back to the caller — and nothing
//! is reported for the thousands of files that were fine. The guard sits
//! at the per-file boundaries of `Session` (parse, interface, the per-file
//! pass, the whole-project pass, finalize), which are the units a crash
//! can be attributed to and the units the rest of the pipeline already
//! knows how to do without (a file that failed to parse).
//!
//! What it cannot catch: a stack overflow or any other abort. Those never
//! unwind; the process dies with the platform's own exit status. The CLI
//! runs its work on large stacks for that reason (rime-cli `main.rs`).
//!
//! Attribution has three parts. The payload is the panic message. The
//! location (`file:line:col` inside Rime) is only visible to the panic
//! hook, so a hook records it per thread while a guard is active — and
//! keeps the default "thread panicked at" output off stderr for a panic
//! that is about to be reported properly. The phase is the innermost
//! `phase` scope the panic unwound through; `timings::time_opt` opens one
//! per rule, so every site that is timed is also named here.

use rime_diag::{Diagnostic, INTERNAL_ERROR};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Once, OnceLock};

/// A caught panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    /// The panic message (`"<non-string panic payload>"` when it had none).
    pub message: String,
    /// `file:line:col` of the panic inside Rime, when the panic was raised
    /// on the guarded thread itself. A panic carried over from another
    /// thread (nested rayon work) has none here; the default hook has
    /// already printed it to stderr in that case.
    pub location: Option<String>,
    /// The innermost `phase` the panic unwound through, e.g. `rule: narrowing`.
    pub phase: Option<&'static str>,
}

thread_local! {
    /// How many `catch` frames are live on this thread.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// (message, location) of the last panic the hook saw on this thread
    /// under a guard. Matched against the payload by message in `catch`, so
    /// a record left behind by a panic that some other frame caught cannot
    /// be mistaken for this one's.
    static LAST_PANIC: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// The innermost phase an in-flight panic has unwound through.
    static CRASH_PHASE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let guarded = DEPTH.try_with(|d| d.get() > 0).unwrap_or(false);
            if !guarded {
                previous(info);
                return;
            }
            if let Some(loc) = info.location() {
                let record = (
                    payload_message(info.payload()),
                    format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
                );
                let _ = LAST_PANIC.try_with(|slot| *slot.borrow_mut() = Some(record));
            }
            // Someone debugging the crash itself wants the backtrace the
            // default hook prints; everyone else gets the diagnostic only.
            if std::env::var_os("RUST_BACKTRACE").is_some_and(|v| v != "0") {
                previous(info);
            }
        }));
    });
}

/// Run `f`; a panic inside it comes back as a `Crash` instead of unwinding
/// further. `f`'s captures are whatever the caller makes of them after a
/// crash — every caller here discards the half-built result and keeps only
/// shared state that is read-only for the duration.
pub fn catch<R>(f: impl FnOnce() -> R) -> Result<R, Crash> {
    install_hook();
    // A stolen rayon job can run a whole other guarded file on this stack
    // while an outer guard waits in a join, so every slot is saved and
    // restored rather than assumed empty.
    let outer_phase = CRASH_PHASE.replace(None);
    DEPTH.set(DEPTH.get() + 1);
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    DEPTH.set(DEPTH.get() - 1);
    let phase = CRASH_PHASE.replace(outer_phase);
    result.map_err(|payload| {
        let message = payload_message(&*payload);
        let location = LAST_PANIC
            .with(|slot| {
                let mut slot = slot.borrow_mut();
                match slot.as_ref() {
                    Some((seen, _)) if *seen == message => slot.take(),
                    _ => None,
                }
            })
            .map(|(_, location)| location);
        Crash { message, location, phase }
    })
}

/// Name the work `f` does, for attribution: a panic that unwinds out of
/// `f` reports `name` as its phase (the innermost such scope wins).
pub fn phase<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    struct Scope(&'static str);
    impl Drop for Scope {
        fn drop(&mut self) {
            if std::thread::panicking() {
                let _ = CRASH_PHASE.try_with(|slot| {
                    if slot.get().is_none() {
                        slot.set(Some(self.0));
                    }
                });
            }
        }
    }
    let _scope = Scope(name);
    f()
}

impl Crash {
    /// The R0901 for a crash while `doing` (a gerund phrase: "checking",
    /// "parsing") this file. `consequence` says what the reader has lost.
    pub fn diagnostic(&self, doing: &str, consequence: &str) -> Diagnostic {
        self.build(
            format!("internal error: Rime crashed while {doing} this file"),
            consequence,
            "please report it with this file",
        )
    }

    /// The R0901 for a crash in work that belongs to no single file.
    pub fn project_diagnostic(&self, doing: &str, consequence: &str) -> Diagnostic {
        self.build(format!("internal error: Rime crashed while {doing}"), consequence, "please report it")
    }

    fn build(&self, message: String, consequence: &str, ask: &str) -> Diagnostic {
        let mut d = Diagnostic::error(INTERNAL_ERROR, message);
        let what = self.message.trim();
        d.notes.push(match &self.location {
            Some(location) => format!("panicked at {location}: {what}"),
            None => format!("panicked: {what}"),
        });
        if let Some(phase) = self.phase {
            d.notes.push(format!("while running `{phase}`"));
        }
        d.notes.push(consequence.to_string());
        d.notes.push(format!(
            "this is a bug in Rime, not in your code — {ask} (set RUST_BACKTRACE=1 for a backtrace)"
        ));
        d
    }
}

/// Test-only fault injection, so the crash path can be exercised end to
/// end on a real binary (and a CI step's handling of the internal-error
/// exit code can be tried without a real bug to hand).
///
/// `RIME_INTERNAL_FAULT` is a comma-separated list of `site:needle`; a
/// guarded site panics when its name matches and the path it is working on
/// contains `needle` (an empty needle matches every path). Sites:
/// `parse`, `interface`, `check`, `exports`, `cycles`, `finalize` here;
/// `custom`, `render`, `run` in the CLI. `phase` is the odd one: it fires
/// inside a timed phase and its needle is matched against the phase name
/// (`phase:rule: locals`), on every file. Unset, this is one atomic load.
pub fn fault(site: &str, path: &Path) {
    static SPEC: OnceLock<Vec<(String, String)>> = OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        std::env::var("RIME_INTERNAL_FAULT")
            .map(|v| {
                v.split(',')
                    .filter(|s| !s.is_empty())
                    .map(|entry| match entry.split_once(':') {
                        Some((site, needle)) => (site.to_string(), needle.to_string()),
                        None => (entry.to_string(), String::new()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    if spec.is_empty() {
        return;
    }
    let shown = path.to_string_lossy();
    if spec.iter().any(|(s, needle)| s == site && shown.contains(needle.as_str())) {
        panic!("injected fault at `{site}` (RIME_INTERNAL_FAULT)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_a_crash_with_message_and_location() {
        let crash = catch(|| -> u32 { panic!("boom {}", 7) }).unwrap_err();
        assert_eq!(crash.message, "boom 7");
        let location = crash.location.expect("a same-thread panic records where it was raised");
        assert!(location.contains("guard.rs"), "{location}");
        assert_eq!(crash.phase, None);
    }

    #[test]
    fn no_panic_passes_the_value_through() {
        assert_eq!(catch(|| 41 + 1), Ok(42));
    }

    #[test]
    fn the_innermost_phase_is_reported() {
        let crash = catch(|| phase("outer", || phase("rule: inner", || panic!("x")))).unwrap_err();
        assert_eq!(crash.phase, Some("rule: inner"));
        // A phase that completed is not blamed for a later panic.
        let crash = catch(|| {
            phase("done", || ());
            panic!("after");
        })
        .unwrap_err();
        assert_eq!(crash.phase, None);
    }

    #[test]
    fn a_nested_guard_does_not_leak_into_the_outer_one() {
        // The shape rayon produces: a second guarded unit runs to
        // completion on the same stack inside the first one's phase.
        let outer = catch(|| {
            phase("outer phase", || {
                let inner = catch(|| phase("inner phase", || panic!("inner")));
                assert_eq!(inner.unwrap_err().phase, Some("inner phase"));
                panic!("outer");
            })
        })
        .unwrap_err();
        assert_eq!(outer.message, "outer");
        assert_eq!(outer.phase, Some("outer phase"));
    }

    #[test]
    fn a_panic_from_another_thread_is_still_caught() {
        // rayon re-raises a job's panic on the thread that waits for it;
        // the hook never ran there, so there is no location — but the
        // message and the phase the unwind crossed survive.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let crash = catch(|| {
            phase("fan out", || pool.install(|| rayon::join(|| (), || panic!("from a worker"))))
        })
        .unwrap_err();
        assert_eq!(crash.message, "from a worker");
        assert_eq!(crash.phase, Some("fan out"));
    }

    #[test]
    fn non_string_payloads_are_named_not_dropped() {
        let crash = catch(|| std::panic::panic_any(17u8)).unwrap_err();
        assert_eq!(crash.message, "<non-string panic payload>");
    }

    #[test]
    fn the_diagnostic_carries_the_evidence() {
        let crash = Crash {
            message: "index out of bounds".into(),
            location: Some("crates/rime-check/src/rules/x.rs:10:5".into()),
            phase: Some("rule: x"),
        };
        let d = crash.diagnostic("checking", "none of this file was checked");
        assert!(d.is_internal_error());
        assert_eq!(d.severity, rime_diag::Severity::Error);
        assert_eq!(d.message, "internal error: Rime crashed while checking this file");
        assert_eq!(d.notes[0], "panicked at crates/rime-check/src/rules/x.rs:10:5: index out of bounds");
        assert_eq!(d.notes[1], "while running `rule: x`");
        assert_eq!(d.notes[2], "none of this file was checked");
        assert!(d.notes[3].contains("bug in Rime"));
    }
}
