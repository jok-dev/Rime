//! R0201: use of a possibly-nil value — the nil-narrowing analysis, and the
//! milestone slice of the product differentiator (plan §3 "Yields as an
//! effect", §5, §7.1/§7.2 both DECIDED 2026-08-20 — do not re-open).
//! R0202: use before guard — the same nil-flow machinery run the other way
//! round (second section of this file).
//!
//! Milestone constraints (decided 2026-08-21):
//!   - only annotated nilable locals participate (`LocalInfo::nilable`);
//!     unannotated locals are inference territory — no claim either way;
//!   - narrowing enters through condition structure (the CFG builder's
//!     `Assume` events: truthiness, `~= nil`, `type(x) == ...`, `and`/`or`/
//!     `not` combinations, the guard-clause and `while x == nil` idioms) and
//!     through assignments whose value is syntactically non-nil. A use
//!     whose every nil-admitting path ends in an unclassifiable write
//!     (call, projection, cast, or — since an `any` annotation never
//!     upgrades an unclassifiable write into a nil claim — any such write
//!     to an `any` local) is reported `conservative` — no path claims nil;
//!   - a local captured by a closure NEVER holds a non-nil fact (plan
//!     §"Aliasing"): the closure is an alias that can write the binding
//!     whenever anything calls it — including inside any call or yield.
//!     Capture is over-approximated by free-name analysis of the nested
//!     bodies — over-poisoning costs precision, never soundness;
//!   - an UNCAPTURED local's fact survives calls and yields: no
//!     alias exists, so nothing the callee runs can rebind it. This is
//!     language semantics, not effect analysis. What a yield invalidates is
//!     the world around a handle (a destroyed Instance is stale, not nil) —
//!     that is the instance-liveness rule's territory, and place-narrowing
//!     (fields) will need the §7.1 kill when it exists;
//!   - field narrowing stays banned (§7.2): only plain locals get facts, so
//!     `t.x` never narrows — users hoist (`local x = t.x`).

use crate::cfg::{Cfg, Event, LocalId, UseKind};
use crate::capture;
use crate::dataflow::{self, Analysis};
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::HashSet;

pub fn check(cfg: &Cfg, module: &hir::Module, body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    let captured = capture::captured_names(module, body);
    let analysis = NarrowAnalysis { captured: &captured, locals: cfg };

    struct Finding {
        local: LocalId,
        span: Span,
        kind: UseKind,
        fact: Fact,
    }
    let mut found: Vec<Finding> = Vec::new();
    dataflow::analyze(cfg, &analysis, |state, ev| {
        if let Event::NonNilUse(id, span, kind) | Event::ConditionalUse(id, span, kind) = ev {
            if cfg.locals[*id].nilable && state[*id] != Fact::NonNil {
                found.push(Finding {
                    local: *id,
                    span: span.clone(),
                    kind: *kind,
                    fact: state[*id],
                });
            }
        }
    });

    for f in found {
        let name = &cfg.locals[f.local].name;
        let is_captured = captured.contains(name);
        diags.push(diag(name, &f.span, f.kind, is_captured, f.fact == Fact::Opaque));
    }

    check_guard_order(cfg, &captured, diags);
}

/// The per-local narrowing fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fact {
    /// Nil is claimed on some path: a bare declaration (`local x: T?`,
    /// `local x: any`), `x = nil`, a `T?` parameter, or
    /// `local x: T? = <unclassifiable>` (all of these reach here as
    /// `Assign(ValueNil::Nil)`). A use is firm.
    Declared,
    /// Nothing proves non-nil and nothing claims nil: every nil-admitting
    /// path's last write was a call, a projection, a cast, or — since an
    /// `any` annotation never upgrades an unclassifiable write into a nil
    /// claim — such a write to an `any` local. Rime cannot classify the
    /// value — a use is reported conservatively.
    Opaque,
    /// Proven non-nil by a dominating condition or a non-nil assignment.
    /// Only uncaptured locals ever hold this, so no call can invalidate it.
    NonNil,
}

struct NarrowAnalysis<'a> {
    captured: &'a HashSet<String>,
    locals: &'a Cfg,
}

impl Analysis for NarrowAnalysis<'_> {
    type Fact = Fact;

    fn entry_fact(&self) -> Fact {
        Fact::Opaque
    }

    /// Must-join: non-nil only if non-nil on every path; a claimed nil on
    /// any path outranks an opaque value on another.
    fn join(&self, a: &Fact, b: &Fact) -> Fact {
        match (a, b) {
            (Fact::NonNil, Fact::NonNil) => Fact::NonNil,
            (Fact::Declared, _) | (_, Fact::Declared) => Fact::Declared,
            _ => Fact::Opaque,
        }
    }

    fn transfer(&self, state: &mut [Fact], ev: &Event) {
        match ev {
            Event::Declare(id) => state[*id] = Fact::Declared, // unobservable for R0201: Declare is emitted only for non-nilable locals
            // The builder classifies assigned values syntactically, so the
            // default-fill idiom (`x = x or 1`) re-establishes the fact
            // instead of resetting it. `Nil` is a claim (firm); `Unknown`
            // is opaque (conservative). The capture filter applies here
            // too: a captured local's value can be rewritten by its closure
            // inside any later call, so a non-nil assignment proves nothing
            // durable.
            Event::Assign(id, vn, _) => {
                state[*id] = match vn {
                    crate::cfg::ValueNil::NonNil if !self.is_captured(*id) => Fact::NonNil,
                    crate::cfg::ValueNil::Nil => Fact::Declared,
                    _ => Fact::Opaque,
                };
            }
            Event::Assume(id) | Event::Asserted(id) => {
                if !self.is_captured(*id) {
                    state[*id] = Fact::NonNil;
                }
            }
            // AnyCall deliberately does NOT kill: only uncaptured
            // locals ever hold NonNil, and no call can reach one.
            _ => {}
        }
    }
}

impl NarrowAnalysis<'_> {
    fn is_captured(&self, id: LocalId) -> bool {
        self.captured.contains(&self.locals.locals[id].name)
    }
}

fn diag(
    name: &str,
    use_span: &Span,
    kind: UseKind,
    is_captured: bool,
    opaque: bool,
) -> Diagnostic {
    let use_label = match kind {
        UseKind::Index => format!("`{name}` is indexed here"),
        UseKind::Method => format!("a method is called on `{name}` here"),
        UseKind::Call => format!("`{name}` is called here"),
        UseKind::Operand => format!("`{name}` is used here in an operation that throws on nil"),
    };
    let d = Diagnostic::error(Code(201), format!("`{name}` may be nil"))
        .with_label(Label::primary(use_span.clone(), use_label));

    if is_captured {
        return d
            .conservative(
                "a closure captures this local, and a captured local can be \
                 rewritten whenever the closure runs — narrowing is disabled for it",
            )
            .with_help(format!(
                "copy to an uncaptured local first — `local v = {name}` — and \
                 check `v` instead"
            ));
    }

    if opaque {
        return d
            .conservative(format!(
                "`{name}` is declared with a type that admits nil, but nothing on \
                 this path claims nil — the value it holds here came from a call \
                 or expression Rime cannot classify"
            ))
            .with_help(format!(
                "guard the use: `if {name} ~= nil then ... end`, or bind the \
                 value to a local declared without `?`"
            ));
    }

    d.with_note(
        "declared with a type that admits nil, and nothing on this path \
         proves it non-nil",
    )
    .with_help(format!("guard the use: `if {name} ~= nil then ... end`"))
}

// --- R0202: use before guard --------------------------------------------------
//
// `x.f` and then `if not x`: the check is either dead (had `x` been nil the
// index would already have thrown) or too late (a nil `x` throws at the
// index and never reaches the check). Either way the code contradicts
// itself, and that contradiction is the whole evidence — so unlike R0201
// this fires for EVERY local, annotated or not. On untyped code it is the
// nil rule that still pays: some of what it finds are certain crashes (a
// field read one line above the nil check of the same local).
//
// Forward must-analysis over the same CFG: a finding needs the check to be
// late-or-dead on every path, so a single path with no unguarded use (a
// legitimate check) silences it. Uses under an `Assume` (the short-circuit
// `x and x.f`, code below a guard clause) are not evidence — a redundant
// second check is a different rule's business.

/// Per-local memory of what happened to it since its last write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Order {
    /// No unguarded non-nil use yet — a check here is legitimate.
    Fresh,
    /// The first unguarded non-nil use on this path.
    Used(Span, UseKind),
    /// A condition proved the local non-nil, or it was assigned a
    /// syntactically non-nil value; uses from here are guarded.
    Guarded,
}

struct GuardOrder {
    /// Locals some closure captures — a call may run the closure and rebind
    /// them, so a use before the call says nothing about the value a check
    /// after it sees (the aliasing rule, read in reverse).
    captured: Vec<LocalId>,
}

impl Analysis for GuardOrder {
    type Fact = Order;

    fn entry_fact(&self) -> Order {
        Order::Fresh
    }

    /// Must-join: `Fresh` on either side wins outright (one legitimate path
    /// is enough to keep quiet). `Used` beats `Guarded` because the used
    /// path is the one the diagnostic points at — on the guarded path the
    /// check is dead, on the used one it is late, and both are the defect.
    /// Two `Used` keep the earlier site.
    fn join(&self, a: &Order, b: &Order) -> Order {
        match (a, b) {
            (Order::Fresh, _) | (_, Order::Fresh) => Order::Fresh,
            (Order::Guarded, Order::Guarded) => Order::Guarded,
            (Order::Used(sa, _), Order::Used(sb, _)) => {
                if sb.start < sa.start {
                    b.clone()
                } else {
                    a.clone()
                }
            }
            (Order::Used(..), Order::Guarded) => a.clone(),
            (Order::Guarded, Order::Used(..)) => b.clone(),
        }
    }

    fn transfer(&self, state: &mut [Order], ev: &Event) {
        match ev {
            Event::Declare(id) => state[*id] = Order::Fresh,
            // A write starts the local's story over. When the value is
            // syntactically non-nil (`x = {}`, `x = x or d`) the uses that
            // follow are guarded by the write itself — R0201 reads such an
            // assignment the same way — unless a closure could rewrite the
            // local before the use (captured: no claim, stay quiet).
            Event::Assign(id, vn, _) => {
                state[*id] = match vn {
                    crate::cfg::ValueNil::NonNil if !self.captured.contains(id) => Order::Guarded,
                    _ => Order::Fresh,
                };
            }
            // `ConditionalUse` (short-circuit rhs, if-expression arm) is
            // deliberately NOT here: the expression may have skipped it,
            // so it proves nothing about every path.
            Event::NonNilUse(id, span, kind) => {
                if state[*id] == Order::Fresh {
                    state[*id] = Order::Used(span.clone(), *kind);
                }
            }
            Event::Assume(id) => state[*id] = Order::Guarded,
            Event::AnyCall(..) => {
                for &id in &self.captured {
                    state[id] = Order::Fresh;
                }
            }
            _ => {}
        }
    }
}

fn check_guard_order(cfg: &Cfg, captured: &HashSet<String>, diags: &mut Vec<Diagnostic>) {
    let analysis = GuardOrder {
        captured: cfg
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| captured.contains(&l.name))
            .map(|(id, _)| id)
            .collect(),
    };

    struct Finding {
        local: LocalId,
        check: Span,
        used: Span,
        kind: UseKind,
    }
    let mut found: Vec<Finding> = Vec::new();
    // An `and`/`or` inside an `if` condition emits its NilCheck twice (the
    // expression walk and the statement both see it): one site, one report.
    let mut seen: HashSet<(LocalId, usize)> = HashSet::new();
    dataflow::analyze(cfg, &analysis, |state, ev| {
        if let Event::NilCheck(id, span) = ev {
            if let Order::Used(used, kind) = &state[*id] {
                if seen.insert((*id, span.start)) {
                    found.push(Finding {
                        local: *id,
                        check: span.clone(),
                        used: used.clone(),
                        kind: *kind,
                    });
                }
            }
        }
    });

    for f in found {
        let info = &cfg.locals[f.local];
        // Declared `T` with nil excluded: the use cannot throw, so the only
        // reading left is "dead check" — valid code, rejected on purpose.
        let dead = info.annotated && !info.nilable;
        diags.push(guard_order_diag(&info.name, f.check, f.used, f.kind, dead));
    }
}

fn guard_order_diag(name: &str, check: Span, used: Span, kind: UseKind, dead: bool) -> Diagnostic {
    let (past, use_label) = match kind {
        UseKind::Index => ("indexed", format!("`{name}` is indexed here — throws if it is nil")),
        UseKind::Method => (
            "used as a method receiver",
            format!("a method is called on `{name}` here — throws if it is nil"),
        ),
        UseKind::Call => ("called", format!("`{name}` is called here — throws if it is nil")),
        UseKind::Operand => (
            "used in an operation that throws on nil",
            format!("`{name}` is used here in an operation that throws on nil"),
        ),
    };
    let d = Diagnostic::error(
        Code(202),
        format!("`{name}` is checked for nil after it was already {past}"),
    )
    .with_label(Label::primary(check.clone(), "nil check here"))
    .with_label(Label::secondary(used, use_label));

    if dead {
        return d
            .conservative(format!(
                "`{name}` is declared with a type that excludes nil, so the use cannot \
                 throw and this check can never fire — a dead check is a \
                 misunderstanding waiting for the type to widen"
            ))
            .with_help(
                "drop the check, or widen the type to `T?` and move the check \
                 above the first use",
            );
    }

    d.with_note(format!(
        "every path from the last assignment of `{name}` to this check reaches the use \
         first, so if `{name}` can be nil the use throws and this check never runs"
    ))
    .with_help(format!(
        "move the check above the first use, or drop it if `{name}` cannot be nil"
    ))
}

#[cfg(test)]
mod tests {
    /// `(line, conservative)` of every R0201 in `src`, in source order.
    fn r0201(src: &str) -> Vec<(usize, bool)> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(201))
            .map(|d| (src[..d.primary_span().start].matches('\n').count() + 1, d.conservative))
            .collect()
    }

    #[test]
    fn assert_is_a_guard() {
        let truthy = "local function f(v: number?)\n\tassert(v)\n\treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(truthy), vec![]);
        let message = "local function f(v: number?)\n\tassert(v, \"v is required\")\n\
                       \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(message), vec![]);
        let not_nil = "local function f(v: number?)\n\tassert(v ~= nil)\n\treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(not_nil), vec![]);
        let type_test = "local function f(v: number?)\n\tassert(type(v) == \"number\")\n\
                         \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(type_test), vec![]);
        let both = "local function f(a: number?, b: number?)\n\tassert(a and b)\n\
                    \treturn a + b\nend\nreturn f\n";
        assert_eq!(r0201(both), vec![]);
        // The value form: `assert` returns its argument once it holds.
        let value = "local function f(v: number?)\n\tlocal w = assert(v)\n\treturn v + w\nend\nreturn f\n";
        assert_eq!(r0201(value), vec![]);
    }

    #[test]
    fn assert_leaves_use_before_check_reports_alone() {
        // R0202 on a check that follows a use: reported with or without an
        // `assert` above the use.
        let r0202 = |src: &str| -> Vec<usize> {
            crate::check_source(src)
                .into_iter()
                .filter(|d| d.code == rime_diag::Code(202))
                .map(|d| src[..d.primary_span().start].matches('\n').count() + 1)
                .collect()
        };
        let plain = "local function f(p)\n\tlocal n = p.Name\n\tif not p then\n\
                     \t\treturn nil\n\tend\n\treturn n\nend\nreturn f\n";
        assert_eq!(r0202(plain), vec![3]);
        let asserted = "local function f(p)\n\tassert(p, \"p is required\")\n\tlocal n = p.Name\n\
                        \tif not p then\n\t\treturn nil\n\tend\n\treturn n\nend\nreturn f\n";
        assert_eq!(r0202(asserted), vec![4]);
    }

    #[test]
    fn assert_that_does_not_prove_the_local_is_not_a_guard() {
        // Before the assert runs.
        let before = "local function f(v: number?)\n\tlocal w = v + 1\n\tassert(v)\n\
                      \treturn w\nend\nreturn f\n";
        assert_eq!(r0201(before), vec![(2, false)]);
        // `a or b` holds when either does.
        let either = "local function f(a: number?, b: number?)\n\tassert(a or b)\n\
                      \treturn a + 1\nend\nreturn f\n";
        assert_eq!(r0201(either), vec![(3, false)]);
        // Asserts something else.
        let other = "local function f(v: number?, ok: boolean)\n\tassert(ok)\n\
                     \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(other), vec![(3, false)]);
        // Only runs on some paths.
        let conditional = "local function f(v: number?, c: boolean)\n\tlocal _ = c and assert(v)\n\
                           \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(conditional), vec![(3, false)]);
        // A local named `assert` is not the builtin.
        let shadowed = "local function f(v: number?, assert: (number?) -> ())\n\tassert(v)\n\
                        \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(shadowed), vec![(3, false)]);
        // Reassigned to nil afterwards.
        let reset = "local function f(v: number?)\n\tassert(v)\n\tv = nil\n\
                     \treturn v + 1\nend\nreturn f\n";
        assert_eq!(r0201(reset), vec![(4, false)]);
    }

    #[test]
    fn default_fill_from_call_is_conservative() {
        // The shape of Roblox's ClickToMoveController.
        let src = "local function f(c: boolean, g: () -> Vector3, h: () -> Vector3?)\n\
                   \tlocal reach: Vector3?\n\
                   \tif c then\n\
                   \t\treach = h()\n\
                   \tend\n\
                   \tif reach == nil then\n\
                   \t\treach = g()\n\
                   \tend\n\
                   \treturn reach.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(9, true)]);
    }

    #[test]
    fn bare_declaration_reachable_is_firm() {
        let src = "local function f(c: boolean, h: () -> Vector3?)\n\
                   \tlocal reach: Vector3?\n\
                   \tif c then\n\
                   \t\treach = h()\n\
                   \tend\n\
                   \treturn reach.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(6, false)]);
    }

    #[test]
    fn bare_any_declaration_reachable_is_firm() {
        // A declaration with no initializer holds nil whatever the
        // annotation says; `any` makes later WRITES opaque, not this.
        let src = "local function f(c: boolean, g: () -> any)\n\
                   \tlocal x: any\n\
                   \tif c then\n\
                   \t\tx = g()\n\
                   \tend\n\
                   \treturn x.y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(6, false)]);
    }

    #[test]
    fn annotated_initializer_from_call_is_firm() {
        let src = "local function f(h: () -> Vector3?)\n\
                   \tlocal reach: Vector3? = h()\n\
                   \treturn reach.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(3, false)]);
    }

    #[test]
    fn nilable_param_is_firm() {
        let src = "local function f(p: Vector3?)\n\
                   \treturn p.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(2, false)]);
    }

    #[test]
    fn nil_literal_write_is_firm() {
        let src = "local function f(g: () -> Vector3)\n\
                   \tlocal x: Vector3? = g()\n\
                   \tx = nil\n\
                   \treturn x.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(4, false)]);
    }

    #[test]
    fn any_local_is_conservative() {
        let src = "local function f(get: () -> any)\n\
                   \tlocal data: any = get()\n\
                   \treturn data.x\n\
                   end\n";
        assert_eq!(r0201(src), vec![(3, true)]);
    }

    #[test]
    fn later_call_write_after_guard_is_conservative() {
        let src = "local function f(g: () -> Vector3, h: () -> Vector3?)\n\
                   \tlocal x: Vector3? = h()\n\
                   \tif x ~= nil then\n\
                   \t\treturn x.Y\n\
                   \tend\n\
                   \tx = g()\n\
                   \treturn x.Y\n\
                   end\n";
        assert_eq!(r0201(src), vec![(7, true)]);
    }

    #[test]
    fn non_nil_assignment_still_clean() {
        let src = "local function f()\n\
                   \tlocal x: { number }? = nil\n\
                   \tx = {}\n\
                   \treturn x[1]\n\
                   end\n";
        assert_eq!(r0201(src), Vec::<(usize, bool)>::new());
    }

    #[test]
    fn opaque_note_names_the_limitation() {
        let src = "local function f(g: () -> Vector3)\n\
                   \tlocal x: Vector3?\n\
                   \tif x == nil then\n\
                   \t\tx = g()\n\
                   \tend\n\
                   \treturn x.Y\n\
                   end\n";
        let d = crate::check_source(src)
            .into_iter()
            .find(|d| d.code == rime_diag::Code(201))
            .expect("one R0201");
        assert!(d.conservative);
        assert!(
            d.notes.iter().any(|n| n.contains("cannot classify")),
            "notes: {:?}",
            d.notes
        );
    }

    #[test]
    fn captured_local_keeps_capture_wording_even_when_opaque() {
        // Branch ordering in `diag`: capture wins over opaque.
        let src = "local function f(g: () -> Vector3)\n\
                   \tlocal x: Vector3?\n\
                   \tlocal function reset()\n\
                   \t\tx = nil\n\
                   \tend\n\
                   \tx = g()\n\
                   \treset()\n\
                   \treturn x.Y\n\
                   end\n";
        let r0201s: Vec<_> = crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(201))
            .collect();
        assert_eq!(r0201s.len(), 1, "expected one R0201");
        let d = &r0201s[0];
        assert!(d.conservative);
        assert!(
            d.notes.iter().any(|n| n.contains("closure captures")),
            "notes: {:?}",
            d.notes
        );
    }

    #[test]
    fn non_nil_on_one_branch_declared_on_other_is_firm() {
        // join(NonNil, Declared) = Declared.
        let src = "local function f(c: boolean)\n\
                   \tlocal x: { number }?\n\
                   \tif c then\n\
                   \t\tx = {}\n\
                   \tend\n\
                   \treturn x[1]\n\
                   end\n";
        assert_eq!(r0201(src), vec![(6, false)]);
    }
}
