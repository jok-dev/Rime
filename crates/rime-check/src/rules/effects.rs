//! R0110 / R0111: effect misuse a statement can prove on its own — a
//! discarded `pcall` result, a table mutated while `pairs` walks it.
//!
//! Detection lives in the CFG builder (`crate::cfg`, `DiscardedPcall` /
//! `PairsMutation`): both recognitions need the builder's scope resolution
//! (unshadowed-global checks, loop-variable identity), so the builder
//! records them during its walk.

use crate::cfg::{Cfg, DiscardedPcall, PairsMutation};
use rime_diag::{Code, Diagnostic, Label};

pub fn check(cfg: &Cfg, diags: &mut Vec<Diagnostic>) {
    for p in &cfg.discarded_pcalls {
        diags.push(discarded_pcall_diag(p));
    }
    for m in &cfg.pairs_mutations {
        diags.push(pairs_mutation_diag(cfg, m));
    }
}

/// R0110: `pcall`/`xpcall` in statement position — the ok flag is discarded,
/// so a failure inside is silently swallowed. Not conservative: the swallowed
/// error IS the bug class, not an approximation of one.
fn discarded_pcall_diag(p: &DiscardedPcall) -> Diagnostic {
    let callee = p.callee;
    Diagnostic::error(Code(110), format!("the result of `{callee}` is discarded"))
        .with_label(Label::primary(
            p.span.clone(),
            "the success flag — and any error — vanish here",
        ))
        .with_note(format!(
            "`{callee}` returns (ok: boolean, ...) — dropping it swallows the error"
        ))
        .with_help(format!(
            "capture the result — `local ok, err = {callee}(...)` — and handle `not ok`"
        ))
}

/// R0111: a single-level write through a local while that same local is being
/// iterated by `pairs`/`ipairs`. Inserting a key during traversal is
/// undefined behavior in Lua — the direct analogue of C++ iterator
/// invalidation. Not conservative: the traversal itself misbehaves at
/// runtime.
///
/// Precision notes (both live in cfg.rs, summarized here for the reader):
///   - a write through the loop's OWN key variable (`t[k] = v`, including
///     `= nil`) targets a field the traversal is visiting, which Lua defines
///     — those are exempt, not flagged;
///   - writes through other names aliasing the table are invisible (no alias
///     analysis this milestone) — silence, never a false positive.
fn pairs_mutation_diag(cfg: &Cfg, m: &PairsMutation) -> Diagnostic {
    let name = &cfg.locals[m.local].name;
    let iter_fn = m.iter_fn;
    Diagnostic::error(
        Code(111),
        format!("table modified while being iterated by `{iter_fn}`"),
    )
    .with_label(Label::primary(
        m.write_span.clone(),
        format!("this write changes `{name}` during the traversal"),
    ))
    .with_label(Label::secondary(
        m.iter_span.clone(),
        format!("`{name}` is being iterated here"),
    ))
    .with_note(format!(
        "adding a key while `{iter_fn}` walks a table is undefined behavior in \
         Lua — the traversal can skip or revisit entries; removing an existing \
         key is legal, but Rime cannot prove this key already exists, so the \
         write may be an insertion"
    ))
    .with_help(format!(
        "collect the changes in a second table (or a list of keys) during \
         the loop and apply them to `{name}` after it ends"
    ))
}
