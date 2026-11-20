//! R0104: dead store — a value assigned to a local that every path
//! overwrites before anything reads it (decided 2026-08-21). Backward
//! liveness on the CFG; the first backward client of the `dataflow`
//! framework. Warning severity, conservative — the code runs fine, the
//! store is wasted at best and a logic slip (wrong variable, misplaced
//! default) at worst, and Rime cannot tell which.
//!
//! Scope decisions:
//!   - only stores that are OVERWRITTEN somewhere are flagged. A final
//!     store nothing ever reads is "dead to the end" — that is the
//!     unused-local family's story (R0013 flags the never-read binding),
//!     and flagging every last write of a used local would be noise;
//!   - a local with zero reads anywhere produces no R0104 at all (same
//!     division of labor: R0013 already points at the declaration);
//!   - captured locals are skipped entirely (capture machinery): a closure can
//!     read the binding inside any call between the store and the
//!     overwrite, so outer-body liveness proves nothing;
//!   - synthetic writes (parameters, loop-variable rebinds at the loop
//!     head, `local x` with no initializer) carry no store span and are
//!     never flagged — flagging a parameter's incoming value would point at
//!     the signature, which reads as the caller's fault;
//!   - `_`-prefixed names are exempt, as everywhere in the unused family;
//!   - the right-hand side still runs — only the STORE is dead, so the fix
//!     wording never says "delete the statement";
//!   - wording tracks the CFG shape (decided 2026-08-21): an overwrite that
//!     precedes the store in the source is reachable only through a loop
//!     back-edge, and the label/note say "next loop iteration" — otherwise
//!     the two spans read as if the tool got the lines backwards. The note
//!     claims "every path overwrites" only when that is literally true;
//!     when some paths just reach the end of the function, it says so.
//!
//! Faint locals are excluded (shipped 2026-08-23): `if not x then x = d
//! end` where nothing else reads `x` — the guard read keeps R0013 quiet
//! and the store used to draw this rule instead, but the honest diagnostic
//! is R0017 ("`x` only feeds itself") at the declaration. `check` takes
//! the faint declarations from `rules::faint` and skips their stores.

use crate::capture;
use crate::cfg::{Cfg, Event, LocalId};
use crate::dataflow::{self, BackwardAnalysis};
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::HashSet;

/// What happens to a local's current value on the paths after a point.
#[derive(Debug, Clone, PartialEq)]
enum Liveness {
    /// Some path reads it before any overwrite — the store matters.
    Live,
    /// No path reads it, and at least one path overwrites it; the span is
    /// the overwriting store when it was explicit (the secondary label),
    /// preferring the earliest site so the diagnostic stays deterministic.
    /// `all_paths` records whether EVERY path overwrites — a join with a
    /// dead-to-end path clears it, and the note wording depends on it.
    Overwritten { span: Option<Span>, all_paths: bool },
    /// Every path reaches an exit without reading or overwriting.
    DeadToEnd,
}

pub fn check(
    cfg: &Cfg,
    module: &hir::Module,
    body: &hir::Body,
    faint: &HashSet<usize>,
    diags: &mut Vec<Diagnostic>,
) {
    let captured = capture::captured_names(module, body);
    let mut read = vec![false; cfg.locals.len()];
    for block in &cfg.blocks {
        for ev in &block.events {
            if let Event::Read(id, _) = ev {
                read[*id] = true;
            }
        }
    }

    struct Finding {
        local: LocalId,
        store_span: Span,
        overwrite_span: Option<Span>,
        all_paths: bool,
    }
    let mut found: Vec<Finding> = Vec::new();
    dataflow::analyze_backward(cfg, &DeadStoreAnalysis, |state, ev| {
        if let Event::Assign(id, _, Some(span)) = ev {
            if let Liveness::Overwritten { span: over, all_paths } = &state[*id] {
                found.push(Finding {
                    local: *id,
                    store_span: span.clone(),
                    overwrite_span: over.clone(),
                    all_paths: *all_paths,
                });
            }
        }
    });

    for f in found {
        let info = &cfg.locals[f.local];
        if info.name.starts_with('_')
            || !read[f.local]
            || captured.contains(&info.name)
            || faint.contains(&info.decl_span.start)
        {
            continue;
        }
        diags.push(diag(&info.name, f.store_span, f.overwrite_span, f.all_paths));
    }
}

struct DeadStoreAnalysis;

impl BackwardAnalysis for DeadStoreAnalysis {
    type Fact = Liveness;

    fn exit_fact(&self) -> Liveness {
        Liveness::DeadToEnd
    }

    /// May-join toward Live: one reading path is enough to keep a store,
    /// so a false "dead" can only come from no path reading — conservative
    /// for a rule that flags deadness.
    fn join(&self, a: &Liveness, b: &Liveness) -> Liveness {
        use Liveness::*;
        match (a, b) {
            (Live, _) | (_, Live) => Live,
            (Overwritten { span: x, all_paths: p }, Overwritten { span: y, all_paths: q }) => {
                Overwritten { span: earlier(x, y), all_paths: *p && *q }
            }
            (Overwritten { span, .. }, DeadToEnd) | (DeadToEnd, Overwritten { span, .. }) => {
                Overwritten { span: span.clone(), all_paths: false }
            }
            (DeadToEnd, DeadToEnd) => DeadToEnd,
        }
    }

    fn transfer(&self, state: &mut [Liveness], ev: &Event) {
        match ev {
            Event::Read(id, _) => state[*id] = Liveness::Live,
            // Any write kills, explicit or synthetic — a loop-head rebind
            // overwrites just as thoroughly as `x = 2`. Only the SPAN
            // depends on explicitness.
            Event::Assign(id, _, span) => {
                state[*id] = Liveness::Overwritten { span: span.clone(), all_paths: true }
            }
            _ => {}
        }
    }
}

/// Deterministic pick between two optional overwrite sites: a labeled site
/// beats an unlabeled one, earlier beats later.
fn earlier(a: &Option<Span>, b: &Option<Span>) -> Option<Span> {
    match (a, b) {
        (Some(x), Some(y)) => {
            // Full lexicographic order so the join stays commutative — the
            // worklist visits predecessors in no fixed order.
            Some(if (x.start, x.end) <= (y.start, y.end) { x.clone() } else { y.clone() })
        }
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    }
}

fn diag(name: &str, store_span: Span, overwrite_span: Option<Span>, all_paths: bool) -> Diagnostic {
    // An overwrite that sits BEFORE the store in the source can only be
    // reached through a loop back-edge — say so, or the labels read as if
    // the tool got the two lines backwards.
    let back_edge = overwrite_span
        .as_ref()
        .is_some_and(|over| over.start < store_span.start);
    let mut d = Diagnostic::warning(Code(104), format!("value assigned to `{name}` is never read"))
        .with_label(Label::primary(
            store_span.clone(),
            "this value is never read",
        ));
    if let Some(over) = overwrite_span {
        let label = if back_edge {
            format!("`{name}` is overwritten here on the next loop iteration")
        } else {
            format!("`{name}` is overwritten here before any read")
        };
        d = d.with_label(Label::secondary(over, label));
    }
    let note = if back_edge {
        "no path reads this value: the next loop iteration overwrites it before any read; \
         the right-hand side still runs, only the stored result is dead"
    } else if all_paths {
        "every path from this assignment overwrites the value before any read; \
         the right-hand side still runs, only the stored result is dead"
    } else {
        "no path reads this value: some paths overwrite it before any read, the rest \
         reach the end of the function without reading it; the right-hand side still \
         runs, only the stored result is dead"
    };
    d.conservative(note).with_help("assign where the value is first needed, or drop this store")
}
