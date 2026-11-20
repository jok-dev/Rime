//! R0101 / R0102 / R0103: the CFG-derived definiteness family — definite
//! assignment, definite return, unreachable code (plan §"CFG-derived
//! checks"). All three fall out of the same graph: R0101 is a dataflow
//! instance, R0102/R0103 read reachability directly.

use crate::cfg::{Cfg, Event, LocalId};
use crate::dataflow::{self, Analysis};
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;

pub fn check(cfg: &Cfg, body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    for read in unassigned_reads(cfg) {
        diags.push(unassigned_diag(cfg, &read));
    }

    let reachable = cfg.reachable();

    // R0102 — only when the annotation excludes nil: unannotated functions
    // are inference territory (no claim yet), and `T?` returns fall off
    // legally. Two causes, each pointed at its own site: genuinely falling
    // off the `end`, and a bare `return` (which hands the caller no values
    // — pointing that one at the `end` sent readers hunting through
    // branches that all return).
    if let (Some(ret), Some(end_span)) = (&body.return_ty, body.end_span.clone()) {
        if !ret.admits_nil {
            if cfg.end_fall_block.is_some_and(|b| reachable[b]) {
                diags.push(missing_return_diag(ret, end_span));
            }
            for (b, span) in &cfg.bare_returns {
                if reachable[*b] {
                    diags.push(bare_return_diag(ret, span.clone()));
                }
            }
        }
    }

    // R0103.
    for (b, block) in cfg.blocks.iter().enumerate() {
        if !reachable[b] {
            if let Some(span) = &block.first_span {
                diags.push(unreachable_diag(span.clone()));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Definedness {
    #[default]
    No,
    Maybe,
    Yes,
}

impl Definedness {
    pub fn join(self, other: Self) -> Self {
        use Definedness::*;
        match (self, other) {
            (Yes, Yes) => Yes,
            (No, No) => No,
            _ => Maybe,
        }
    }
}

/// A read of a local on a path where it may hold nothing.
#[derive(Debug)]
pub struct UnassignedRead {
    pub local: LocalId,
    pub span: rime_diag::Span,
    /// `Maybe` = some path assigns, some does not. `No` = no path does.
    pub state: Definedness,
}

/// Must-analysis: defined only if assigned on every path.
struct DefinednessAnalysis;

impl Analysis for DefinednessAnalysis {
    type Fact = Definedness;

    fn entry_fact(&self) -> Definedness {
        Definedness::No
    }

    fn join(&self, a: &Definedness, b: &Definedness) -> Definedness {
        a.join(*b)
    }

    fn transfer(&self, state: &mut [Definedness], ev: &Event) {
        match *ev {
            Event::Declare(id) => state[id] = Definedness::No,
            Event::Assign(id, _, _) => state[id] = Definedness::Yes,
            // Whether the closure has run by a later read is not knowable
            // here, so the local counts as assigned: silence, never a
            // wrong report. A read before the closure exists still reports.
            Event::ClosureAssign(id) => state[id] = Definedness::Yes,
            _ => {}
        }
    }
}

/// Definite assignment over a CFG (R0101).
pub fn unassigned_reads(cfg: &Cfg) -> Vec<UnassignedRead> {
    let mut found = Vec::new();
    dataflow::analyze(cfg, &DefinednessAnalysis, |state, ev| {
        if let Event::Read(id, span) = ev {
            if state[*id] != Definedness::Yes {
                found.push(UnassignedRead {
                    local: *id,
                    span: span.clone(),
                    state: state[*id],
                });
            }
        }
    });
    found
}

fn unassigned_diag(cfg: &Cfg, read: &UnassignedRead) -> Diagnostic {
    let info = &cfg.locals[read.local];
    let name = &info.name;
    let (message, note) = match read.state {
        Definedness::Maybe => (
            format!("`{name}` may be unassigned here"),
            format!("some paths from the declaration assign `{name}`, but at least one does not"),
        ),
        _ => (
            format!("`{name}` is never assigned before this read"),
            format!("no path from the declaration assigns `{name}` a value"),
        ),
    };
    Diagnostic::error(Code(101), message)
        .with_label(Label::primary(
            read.span.clone(),
            "read while possibly unassigned",
        ))
        .with_label(Label::secondary(
            info.decl_span.clone(),
            format!("`{name}` declared here without a value"),
        ))
        .with_note(note)
        .with_help(format!(
            "initialize `{name}` at its declaration, or assign it on every branch"
        ))
}

fn missing_return_diag(ret: &hir::Ty, end_span: Span) -> Diagnostic {
    Diagnostic::error(Code(102), "not all paths return a value")
        .with_label(Label::primary(
            end_span,
            "control can fall off this `end` without returning",
        ))
        .with_label(Label::secondary(ret.span.clone(), "return type declared here"))
        .with_note("in Luau, falling off the end of a function silently returns nil")
        .with_help("return a value on every path")
}

fn bare_return_diag(ret: &hir::Ty, span: Span) -> Diagnostic {
    Diagnostic::error(Code(102), "`return` with no values in a function that declares them")
        .with_label(Label::primary(span, "the caller receives nil for every declared value"))
        .with_label(Label::secondary(ret.span.clone(), "return type declared here"))
        .with_help("return values here, or make the declared types optional")
}

fn unreachable_diag(span: Span) -> Diagnostic {
    Diagnostic::warning(Code(103), "unreachable code")
        .with_label(Label::primary(span, "this statement can never run"))
        .with_note("every path exits before reaching it")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definite_assignment_joins() {
        assert_eq!(Definedness::Yes.join(Definedness::No), Definedness::Maybe);
        assert_eq!(Definedness::Yes.join(Definedness::Yes), Definedness::Yes);
    }

    /// (line, code) of every R0101/R0102/R0103 diagnostic.
    fn definite_diags(src: &str) -> Vec<(usize, u16)> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| (101..=103).contains(&d.code.0))
            .map(|d| (src[..d.primary_span().start].matches('\n').count() + 1, d.code.0))
            .collect()
    }

    #[test]
    fn local_assigned_by_a_closure_is_not_reported() {
        let named = "local function f(): number\n\tlocal x: number\n\tlocal function set()\n\
                     \t\tx = 1\n\tend\n\tset()\n\treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(named), vec![]);
        let callback = "local function f(run: (() -> ()) -> ()): number\n\tlocal x: number\n\
                        \trun(function()\n\t\tx = 1\n\tend)\n\treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(callback), vec![]);
        let nested = "local function f(run: (() -> ()) -> ()): number\n\tlocal x: number\n\
                      \trun(function()\n\t\trun(function()\n\t\t\tx = 1\n\t\tend)\n\tend)\n\
                      \treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(nested), vec![]);
        let method = "local T = {}\nlocal function f(): number\n\tlocal x: number\n\
                      \tfunction T.set()\n\t\tx = 1\n\tend\n\tT.set()\n\treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(method), vec![]);
    }

    #[test]
    fn read_before_the_assigning_closure_exists_is_still_reported() {
        let src = "local function f(): number\n\tlocal x: number\n\tlocal y = x\n\
                   \tlocal function set()\n\t\tx = 1\n\tend\n\tset()\n\treturn x + y\nend\nreturn f\n";
        assert_eq!(definite_diags(src), vec![(3, 101)]);
    }

    #[test]
    fn closure_that_does_not_assign_the_local_changes_nothing() {
        let reads = "local function f(): number\n\tlocal x: number\n\tlocal function get()\n\
                     \t\treturn x\n\tend\n\tget()\n\treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(reads), vec![(7, 101)]);
        // The closure assigns its own `x`, not the enclosing one.
        let own = "local function f(): number\n\tlocal x: number\n\tlocal function set()\n\
                   \t\tlocal x = 0\n\t\tx = 1\n\t\treturn x\n\tend\n\tset()\n\treturn x\nend\nreturn f\n";
        assert_eq!(definite_diags(own), vec![(9, 101)]);
    }

    #[test]
    fn endless_loop_that_returns_from_inside_does_not_fall_off_the_end() {
        let while_true = "local function f(n: number): number\n\twhile true do\n\t\tn += 1\n\
                          \t\tif n > 10 then\n\t\t\treturn n\n\t\tend\n\tend\nend\nreturn f\n";
        assert_eq!(definite_diags(while_true), vec![]);
        let repeat_false = "local function f(n: number): number\n\trepeat\n\t\tn += 1\n\
                            \t\tif n > 10 then\n\t\t\treturn n\n\t\tend\n\tuntil false\nend\nreturn f\n";
        assert_eq!(definite_diags(repeat_false), vec![]);
    }

    #[test]
    fn endless_loop_left_by_break_still_falls_off_the_end() {
        let src = "local function f(n: number): number\n\twhile true do\n\t\tn += 1\n\
                   \t\tif n > 10 then\n\t\t\tbreak\n\t\tend\n\tend\nend\nreturn f\n";
        assert_eq!(definite_diags(src), vec![(8, 102)]);
        let repeat_false = "local function f(n: number): number\n\trepeat\n\t\tn += 1\n\
                            \t\tif n > 10 then\n\t\t\tbreak\n\t\tend\n\tuntil false\nend\nreturn f\n";
        assert_eq!(definite_diags(repeat_false), vec![(8, 102)]);
    }

    #[test]
    fn loop_on_a_real_condition_still_falls_off_the_end() {
        let src = "local function f(n: number): number\n\twhile n < 10 do\n\t\tn += 1\n\
                   \t\tif n > 5 then\n\t\t\treturn n\n\t\tend\n\tend\nend\nreturn f\n";
        assert_eq!(definite_diags(src), vec![(8, 102)]);
        let repeat_cond = "local function f(n: number): number\n\trepeat\n\t\tn += 1\n\
                           \t\tif n > 5 then\n\t\t\treturn n\n\t\tend\n\tuntil n > 10\nend\nreturn f\n";
        assert_eq!(definite_diags(repeat_cond), vec![(8, 102)]);
    }

    #[test]
    fn code_after_an_endless_loop_is_unreachable() {
        let src = "local function f(n: number): number\n\twhile true do\n\t\tn += 1\n\
                   \t\tif n > 10 then\n\t\t\treturn n\n\t\tend\n\tend\n\tprint(n)\nend\nreturn f\n";
        assert_eq!(definite_diags(src), vec![(8, 103)]);
    }

    #[test]
    fn assert_false_ends_the_path() {
        let src = "local function f(kind: string): number\n\tif kind == \"a\" then\n\t\treturn 1\n\
                   \tend\n\tassert(false, \"unknown kind\")\nend\nreturn f\n";
        assert_eq!(definite_diags(src), vec![]);
        // A condition Rime cannot evaluate is not a divergence.
        let cond = "local function f(kind: string): number\n\tif kind == \"a\" then\n\t\treturn 1\n\
                    \tend\n\tassert(kind == \"b\", \"unknown kind\")\nend\nreturn f\n";
        assert_eq!(definite_diags(cond), vec![(6, 102)]);
        // A local named `assert` is not the builtin.
        let shadowed = "local function f(assert, kind: string): number\n\tif kind == \"a\" then\n\
                        \t\treturn 1\n\tend\n\tassert(false, \"unknown kind\")\nend\nreturn f\n";
        assert_eq!(definite_diags(shadowed), vec![(6, 102)]);
    }
}
