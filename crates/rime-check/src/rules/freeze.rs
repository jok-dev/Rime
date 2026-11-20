//! R0041 / R0042: operations that throw on a frozen table, proven frozen on
//! every path (must-analysis, plan §7.3). Not conservative — a guaranteed
//! runtime throw, the exact theorem territory.
//!
//! `FrozenState` is also the narrowing milestone's licensing fact: only
//! `Deep` lets narrowing survive a yield (plan §5), and flow analysis alone
//! only ever proves `Shallow` — `Deep` waits for the type system.

use crate::cfg::{Cfg, Event, LocalId};
use crate::dataflow::{self, Analysis};
use rime_diag::{Code, Diagnostic, Label};

pub fn check(cfg: &Cfg, src: &str, diags: &mut Vec<Diagnostic>) {
    for violation in frozen_violations(cfg) {
        diags.push(frozen_diag(cfg, &violation, src));
    }
}

/// Whether a value is known immutable, and how deeply.
///
/// `Deep` is the one that matters: it is what licenses narrowing to survive a
/// yield (plan §5). `Shallow` does not, because a nested table can still be
/// mutated underneath you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrozenState {
    #[default]
    No,
    Shallow,
    Deep,
}

impl FrozenState {
    /// Meet, for control-flow joins: frozen only if frozen on every path.
    /// This is the must-analysis that answers plan §7.3 (conditional freeze).
    pub fn join(self, other: Self) -> Self {
        use FrozenState::*;
        match (self, other) {
            (Deep, Deep) => Deep,
            (No, _) | (_, No) => No,
            _ => Shallow,
        }
    }
}

/// Per-local frozen fact: how frozen, and where the freeze happened (the
/// secondary label of every R04xx diagnostic). Invariant: `site` is Some
/// exactly when `state != No`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct FrozenFact {
    state: FrozenState,
    site: Option<rime_diag::Span>,
}

impl FrozenFact {
    /// Join at merge points: `FrozenState::join` is the must-analysis
    /// (plan §7.3 — frozen only if frozen on every path). When both paths
    /// froze at different sites, keep the earlier one; either is a true
    /// freeze site, and earliest reads best in the diagnostic.
    fn join(&self, other: &Self) -> Self {
        let state = self.state.join(other.state);
        let site = match state {
            FrozenState::No => None,
            _ => match (&self.site, &other.site) {
                (Some(a), Some(b)) => Some(if a.start <= b.start { a.clone() } else { b.clone() }),
                (Some(a), None) => Some(a.clone()),
                (None, b) => b.clone(),
            },
        };
        FrozenFact { state, site }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrozenViolationKind {
    /// `t.x = 1` / `t[k] = v` / compound assign through a frozen binding.
    Write,
    /// `setmetatable(t, ...)` on a frozen binding.
    SetMeta,
}

/// A statically-provable runtime throw: an operation on a binding that is
/// frozen on EVERY path reaching it.
#[derive(Debug)]
pub struct FrozenViolation {
    pub local: LocalId,
    pub kind: FrozenViolationKind,
    /// The write (or setmetatable call) that throws.
    pub span: rime_diag::Span,
    /// Where the table was frozen.
    pub freeze_span: rime_diag::Span,
}

struct FrozenAnalysis;

impl Analysis for FrozenAnalysis {
    type Fact = FrozenFact;

    fn entry_fact(&self) -> FrozenFact {
        FrozenFact::default()
    }

    fn join(&self, a: &FrozenFact, b: &FrozenFact) -> FrozenFact {
        a.join(b)
    }

    fn transfer(&self, state: &mut [FrozenFact], ev: &Event) {
        match ev {
            // A (re)declaration or reassignment rebinds the name to a possibly
            // different object: the fact resets.
            Event::Declare(id) | Event::Assign(id, _, _) => state[*id] = FrozenFact::default(),
            Event::Freeze(id, span) => {
                // `table.freeze` is shallow: everything the flow
                // analysis can prove today is Shallow. Nothing produces Deep yet —
                // Deep is reserved for the DeepFrozen-by-type computation that
                // licenses narrowing across yields (plan §5). Re-freezing is a
                // runtime no-op: keep the first site.
                if state[*id].state == FrozenState::No {
                    state[*id] = FrozenFact {
                        state: FrozenState::Shallow,
                        site: Some(span.clone()),
                    };
                }
            }
            _ => {}
        }
    }
}

/// Freeze flow over a CFG (R0041/R0042).
///
/// Because the join is a must-analysis, a maybe-frozen write (frozen on one
/// path, not the other) is NOT reported — catching it needs a may-analysis,
/// a documented milestone gap, not a decision.
pub fn frozen_violations(cfg: &Cfg) -> Vec<FrozenViolation> {
    let mut found = Vec::new();
    dataflow::analyze(cfg, &FrozenAnalysis, |state, ev| {
        let (id, span, kind) = match ev {
            Event::TableWrite(id, span) => (id, span, FrozenViolationKind::Write),
            Event::SetMeta(id, span) => (id, span, FrozenViolationKind::SetMeta),
            _ => return,
        };
        if let Some(freeze_span) = state[*id].site.clone() {
            found.push(FrozenViolation {
                local: *id,
                kind,
                span: span.clone(),
                freeze_span,
            });
        }
    });
    found
}

fn frozen_diag(cfg: &Cfg, v: &FrozenViolation, src: &str) -> Diagnostic {
    let name = &cfg.locals[v.local].name;
    let freeze_line = line_of(src, v.freeze_span.start);

    let diag = match v.kind {
        FrozenViolationKind::Write => {
            Diagnostic::error(Code(41), "cannot write to a frozen table")
                .with_label(Label::primary(
                    v.span.clone(),
                    format!("`{name}` is frozen when this write runs"),
                ))
                .with_note(format!(
                    "frozen at line {freeze_line}: `table.freeze` freezes the table itself, \
                     so every binding to it — not just the one in the freeze call — \
                     rejects writes from then on"
                ))
                .with_help(format!(
                    "move this write before the freeze, or copy first with \
                     `table.clone({name})` and write to the copy"
                ))
        }
        FrozenViolationKind::SetMeta => {
            Diagnostic::error(Code(42), "cannot set a metatable on a frozen table")
                .with_label(Label::primary(
                    v.span.clone(),
                    format!("`{name}` is frozen when this call runs"),
                ))
                .with_note(format!(
                    "frozen at line {freeze_line}: `setmetatable` counts as a write, \
                     so it throws on a frozen table"
                ))
                .with_help("set the metatable first and freeze last")
        }
    };
    diag.with_label(Label::secondary(
        v.freeze_span.clone(),
        format!("`{name}` frozen here"),
    ))
}

/// 1-based line number of a byte offset — for "frozen at line N" notes.
fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_freeze_is_must_analysis() {
        assert_eq!(FrozenState::Deep.join(FrozenState::No), FrozenState::No);
        assert_eq!(FrozenState::Deep.join(FrozenState::Deep), FrozenState::Deep);
        assert_eq!(
            FrozenState::Deep.join(FrozenState::Shallow),
            FrozenState::Shallow
        );
    }
}
