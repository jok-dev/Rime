//! Control-flow graph, built from the parser-agnostic HIR.
//!
//! Five analyses ride on it and none of them depend on the aliasing question
//! that is still open (plan §7.1):
//!
//!   - definite assignment          R01xx
//!   - definite return              R01xx
//!   - unreachable code             R01xx
//!   - yield-point narrowing kills  R02xx
//!   - freeze flow-sensitivity      R04xx
//!
//! Luau specifics that shape this:
//!   - `repeat ... until c` — the condition sees locals declared in the body
//!   - `break`/`continue`   — `continue` targets the loop latch, not the head
//!   - `goto` does not exist in Luau; do not build for it
//!   - a call may yield; every call site is potentially a suspension edge
//!   - `error()` diverges — successors are empty (`assert(false)` still
//!     needs constant folding)
//!
//! Milestone limitations, all deliberate (documented so they are not mistaken
//! for decisions):
//!   - closures are checked as independent bodies; a closure's reads/writes of
//!     outer locals are not yet modelled (blocked on §7.1 either way)
//!   - `local a, b = f()` treats every name as assigned regardless of arity

use crate::capture;
use rime_diag::Span;
use rime_hir as hir;
use std::collections::HashMap;

fn union(mut a: Vec<LocalId>, b: Vec<LocalId>) -> Vec<LocalId> {
    for id in b {
        if !a.contains(&id) {
            a.push(id);
        }
    }
    a
}

fn intersect(a: Vec<LocalId>, b: Vec<LocalId>) -> Vec<LocalId> {
    a.into_iter().filter(|id| b.contains(id)).collect()
}

pub type LocalId = usize;
pub type BlockId = usize;

#[derive(Debug, Clone)]
pub enum Event {
    /// `local x` with no initializer and a non-nilable annotation: `x` exists
    /// but reading it is a type violation until assigned.
    Declare(LocalId),
    /// A closure that assigns the local is created here. The local was
    /// declared without a value (`Declare`); from this point a call to the
    /// closure may have assigned it, and when the closure runs is outside
    /// the per-function graph. Definite assignment (R0101) makes no claim
    /// about the local past this event. No other analysis reads it.
    ClosureAssign(LocalId),
    /// Any write: initializer, assignment, loop variable, parameter. The
    /// `ValueNil` is a build-time syntactic classification of the assigned
    /// value, so the default-fill idiom (`x = x or 1`,
    /// `x = if type(x) == "number" then x else 1`) re-establishes non-nil
    /// instead of resetting it.
    ///
    /// The `Option<Span>` marks an explicit user store — a declaration
    /// initializer, an assignment target, a `local function` binding — and
    /// is where a dead-store diagnostic (R0104) anchors. `None` for
    /// synthetic writes: parameters, loop variables, a declaration without
    /// initializer, and the `function f()` root assignment (whose only
    /// available span is the whole statement).
    Assign(LocalId, ValueNil, Option<Span>),
    /// A use site. Span points at the identifier for the diagnostic.
    Read(LocalId, Span),
    /// `table.freeze(x)` in statement or initializer position: the object `x`
    /// names is frozen from here on (freeze returns the SAME table, so the
    /// argument binding is retyped, not just the result — plan §5 wrinkle).
    /// Span is the freeze site, for the secondary label.
    Freeze(LocalId, Span),
    /// A single-level write through a local: `t.x = 1`, `t[k] = v`, or the
    /// compound forms. Deeper chains (`t.a.b = 1`) mutate a *nested* table,
    /// which a shallow freeze does not protect, so they are not events.
    /// Span is the whole assignment, for the primary label.
    TableWrite(LocalId, Span),
    /// `setmetatable(x, ...)` where `x` resolves to a local. Throws at
    /// runtime if `x` is frozen — class construction must freeze last.
    SetMeta(LocalId, Span),
    /// The condition on the edge into this block proved the local non-nil
    /// (narrowing milestone). Placed only in blocks with a single incoming
    /// edge — the builder inserts a dedicated pass-through block when the
    /// natural target has other predecessors — so the fact never leaks onto
    /// a path the condition does not dominate.
    Assume(LocalId),
    /// `assert(cond, ...)` returned, and `cond` being true proves the local
    /// non-nil (same condition shapes as `Assume`). Emitted right after the
    /// call's `AnyCall`, mid-block: every path past this point ran the
    /// assert. Its own event rather than `Assume` so that only R0201 reads
    /// it — R0202's use-before-check reports are unchanged by an `assert`.
    Asserted(LocalId),
    /// Any call, in evaluation order. Conservative §7.1 (decided
    /// 2026-08-20): a call invalidates every active narrowing — it can run
    /// arbitrary code, and if it yields, the world changes underneath the
    /// thread. `CallKind` says whether the callee is a recognized yielder,
    /// for the diagnostic's wording only; the kill is identical.
    AnyCall(Span, CallKind),
    /// A use of the local in a position that requires non-nil: index base,
    /// call base, method receiver, or an operand of an operator that throws
    /// on nil. The narrowing analysis decides whether the fact in force
    /// proves the use safe.
    NonNilUse(LocalId, Span, UseKind),
    /// A `NonNilUse` that evaluates only on some paths through its own
    /// expression: the right operand of `and`/`or`, or an if-expression
    /// arm. R0201 treats it exactly like `NonNilUse` (a use is a use);
    /// R0202 ignores it, because "used on every path to the check" is not
    /// established by a use the expression may have skipped.
    ConditionalUse(LocalId, Span, UseKind),
    /// A condition tests the local for nil — `x`, `x == nil`, `type(x) ==
    /// ...` — in an `if`/`elseif`/`while` condition, an if-expression arm,
    /// or as the guard side of `and`/`or`. Span is the tested name's
    /// occurrence. R0202 asks whether an unguarded non-nil use already
    /// preceded it on every path.
    NilCheck(LocalId, Span),
}

/// What the assigned value's nil-ness is knowable to be from syntax alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueNil {
    /// Provably not nil: a non-nil literal, table or closure constructor,
    /// an operator that never yields nil, an `or` with a non-nil fallback,
    /// an if-expression whose every arm is non-nil (a Name arm counts when
    /// the arm's own condition proves it).
    NonNil,
    /// Nil is claimed: the `nil` literal, a bare declaration (`local x: T?`,
    /// `local x: any` — no initializer), a `T?` parameter, or a
    /// `local x: T? = e` whose `e` is otherwise unclassifiable — the
    /// annotation is the user's statement that THIS value may be nil. R0201
    /// reports a use under this fact firmly.
    Nil,
    /// Everything else — calls, projections, plain names, casts, and (since
    /// an `any` annotation never upgrades an unclassifiable write into a
    /// nil claim) any such write to an `any`-annotated local. Nobody
    /// claimed nil; Rime just cannot classify the value. R0201 reports a
    /// use under this fact as conservative.
    Unknown,
}

/// An annotation is the user's claim about an initializer Rime cannot
/// classify: `local x: T? = e` with unclassifiable `e`, and a `T?`
/// parameter, say "this may be nil" — a firm fact. `any` claims nothing, so
/// its unclassifiable writes stay opaque. The no-initializer case is
/// decided by the caller (a bare declaration holds nil whatever the
/// annotation); a classified initializer (`= {}`, `= a or 1`, `= nil`)
/// keeps its own class.
fn declared_nil(decl: &hir::LocalDecl, vn: ValueNil) -> ValueNil {
    match (&decl.ty, vn) {
        (Some(t), ValueNil::Unknown) if t.admits_nil && !t.any => ValueNil::Nil,
        _ => vn,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// The callee name is on the recognized yielder list (`task.wait`,
    /// `WaitForChild`, `InvokeServer`, DataStore methods, `signal:Wait`...).
    KnownYield,
    /// Everything else — still kills narrowing (any unknown call is a
    /// potential yield under the conservative rule).
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    /// `x.f` / `x[k]`.
    Index,
    /// `x(...)`.
    Call,
    /// `x:m(...)`.
    Method,
    /// Operand of arithmetic/concat/ordering, which throw on nil.
    Operand,
}

/// R0110: a statement-position call to the unshadowed global `pcall` or
/// `xpcall`. Statement position IS the discard — the ok flag never binds to
/// anything, so a failure inside is silently swallowed. Any use of the
/// result (local init, assignment RHS, condition, argument, return expr) is
/// a different statement kind and produces no finding.
///
/// Structural, not dataflow: collected during the CFG walk because the walk
/// already owns scope (shadowing) — no fixpoint needed.
#[derive(Debug)]
pub struct DiscardedPcall {
    /// `"pcall"` or `"xpcall"`.
    pub callee: &'static str,
    /// The call statement.
    pub span: Span,
}

/// R0111: a single-level write through a local (`t[k] = v`, `t.x = v`,
/// compound assign) while a `for ... in pairs(t)`/`ipairs(t)` generic-for is
/// iterating that same local. Inserting a key during traversal is undefined
/// behavior in Lua.
///
/// Writes through OTHER names aliasing the same table are invisible here —
/// no alias analysis this milestone, so that is accepted silence (a false
/// negative, never a false positive).
#[derive(Debug)]
pub struct PairsMutation {
    pub local: LocalId,
    /// `"pairs"` or `"ipairs"`.
    pub iter_fn: &'static str,
    /// The write statement — the primary label.
    pub write_span: Span,
    /// The `pairs(t)` call in the `for` header — the secondary label.
    pub iter_span: Span,
}

/// R0011: a local declaration re-using a name still visible from an enclosing
/// scope of the same body. Valid Luau (the new binding simply wins), rejected
/// as bug-prone by default — a later read silently picks whichever binding is
/// innermost. `_`-prefixed names are exempt (deliberate discards); a closure
/// re-declaring an outer FUNCTION's local is invisible here (bodies are
/// walked independently — documented milestone gap).
#[derive(Debug)]
pub struct Shadow {
    /// The new (shadowing) binding.
    pub local: LocalId,
    /// The binding it hides.
    pub prev: LocalId,
}

#[derive(Debug, Default)]
pub struct BasicBlock {
    pub events: Vec<Event>,
    pub succs: Vec<BlockId>,
    /// Span of the first statement placed in this block — what an
    /// unreachable-code warning points at.
    pub first_span: Option<Span>,
}

/// What kind of declaration produced a local — the unused-* rules (R0013/14/
/// 15) pick their code and wording by it; no analysis branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// Function parameter.
    Param,
    /// `local x`, in any form including multi-assignment names.
    Plain,
    /// A `for` variable — numeric counter or generic-for name.
    Loop,
    /// The binding of a `local function f`.
    Function,
}

#[derive(Debug)]
pub struct LocalInfo {
    pub name: String,
    pub decl_span: Span,
    pub kind: LocalKind,
    /// The declared annotation admits nil (`T?`, `nil`, `any`...). Only
    /// these locals participate in nil-narrowing: an unannotated local is
    /// inference territory (no claim), and a non-nilable one cannot hold
    /// nil legally in the first place.
    pub nilable: bool,
    /// The declaration carries an annotation at all. With `nilable` this
    /// separates "declared `T`" (nil excluded — a nil check on it is dead)
    /// from "unannotated" (no claim either way).
    pub annotated: bool,
}

#[derive(Debug)]
pub struct Cfg {
    pub blocks: Vec<BasicBlock>,
    pub entry: BlockId,
    /// Synthetic block reached only by falling off the end of the body (or by
    /// an empty `return`). `return <expr>` exits the graph directly, so
    /// "is `fall_off` reachable" IS the definite-return question.
    pub fall_off: BlockId,
    pub locals: Vec<LocalInfo>,
    /// R0110 findings, in source order (see `DiscardedPcall`).
    pub discarded_pcalls: Vec<DiscardedPcall>,
    /// R0111 findings, in source order (see `PairsMutation`).
    pub pairs_mutations: Vec<PairsMutation>,
    /// R0011 findings, in source order (see `Shadow`).
    pub shadows: Vec<Shadow>,
    /// Bare `return` sites (block, span) — each is its own R0102 cause when
    /// its block is reachable and the return type excludes nil.
    pub bare_returns: Vec<(BlockId, Span)>,
    /// The block whose LAST statement can fall off the end of the body —
    /// `None` when every path exits explicitly. R0102's classic
    /// points-at-`end` diagnostic fires only when this block is reachable.
    pub end_fall_block: Option<BlockId>,
}

impl Cfg {
    pub fn reachable(&self) -> Vec<bool> {
        let mut seen = vec![false; self.blocks.len()];
        let mut stack = vec![self.entry];
        seen[self.entry] = true;
        while let Some(b) = stack.pop() {
            for &s in &self.blocks[b].succs {
                if !seen[s] {
                    seen[s] = true;
                    stack.push(s);
                }
            }
        }
        seen
    }
}

struct LoopCtx {
    continue_target: BlockId,
    break_target: BlockId,
}

/// Callee names that suspend the running thread (a hardcoded stdlib
/// list; any unknown call is a potential yield anyway under the conservative
/// rule — this list only improves the diagnostic's wording). Matched against
/// the last name of the callee: the method name (`plr:WaitForChild`), the
/// dot field (`task.wait`), or the bare global (`wait`).
const KNOWN_YIELDERS: &[&str] = &[
    "wait",
    "Wait",
    "WaitForChild",
    "WaitForChildWhichIsA",
    "InvokeServer",
    "InvokeClient",
    "yield",
    "GetAsync",
    "SetAsync",
    "UpdateAsync",
    "RemoveAsync",
    "IncrementAsync",
    "GetSortedAsync",
    "PromptPurchaseAsync",
];

/// See through `-`/`#` layers. Flow analyses treat the value-producing
/// unary operators as transparent reads of their operand — evaluating one
/// on nil throws, so every claim made of the operand position holds.
/// Which truth value of a nil test proves the tested local non-nil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proves {
    WhenTrue,
    WhenFalse,
}

/// One recognized leaf of a condition that says something about a local
/// being nil (see `Builder::nil_test_leaf`).
struct NilTest {
    id: LocalId,
    /// The tested name's occurrence.
    span: Span,
    proves: Proves,
    /// The test is ABOUT nil (`x`, `x == nil`, `type(x) == "nil"`), as
    /// opposed to a type test that merely excludes it (`type(x) == "table"`
    /// still discriminates tables from strings). Only pure tests are R0202
    /// check sites; both kinds feed narrowing.
    pure: bool,
}

pub(crate) fn peel_unop(mut e: &hir::Expr) -> &hir::Expr {
    while let hir::Expr::UnOp { operand, .. } = e {
        e = operand;
    }
    e
}

/// A literal that is truthy whatever runs: `true`, a number, a string.
/// Only literals — a name is not evaluated, so `while running do` keeps
/// its exit edge.
fn always_truthy(e: &hir::Expr) -> bool {
    match e {
        hir::Expr::Literal { prim: hir::Prim::Boolean, text, .. } => text.as_deref() == Some("true"),
        hir::Expr::Literal { prim: hir::Prim::Number | hir::Prim::String, .. } => true,
        _ => false,
    }
}

/// The literals `false` and `nil`.
fn always_falsy(e: &hir::Expr) -> bool {
    match e {
        hir::Expr::Literal { prim: hir::Prim::Boolean, text, .. } => text.as_deref() == Some("false"),
        hir::Expr::Literal { prim: hir::Prim::Nil, .. } => true,
        _ => false,
    }
}

/// Classify a call: is the callee a recognized yielder? Used for the
/// narrowing diagnostic's wording and as the yield-effect pass's base facts.
pub(crate) fn call_kind(base: &hir::Expr, method: Option<&str>) -> CallKind {
    let last_name = match (base, method) {
        (_, Some(m)) => Some(m),
        (hir::Expr::Index { key: None, name: Some(f), .. }, None) => Some(f.as_str()),
        (hir::Expr::Name { name, .. }, None) => Some(name.as_str()),
        _ => None,
    };
    match last_name {
        Some(n) if KNOWN_YIELDERS.contains(&n) => CallKind::KnownYield,
        _ => CallKind::Unknown,
    }
}

/// One level of `for ... in pairs(t)` the walker is currently inside — the
/// stack `pairs_stack` is consulted by `projection_table_write` for R0111.
/// Nested loops each push a frame, so a write to ANY currently-iterated
/// local is caught, not just the innermost.
struct PairsFrame {
    /// The iterated table, as the LocalId it resolved to BEFORE the loop
    /// scope opened — a re-declared `t` inside the body is a different id
    /// and never matches.
    local: LocalId,
    iter_fn: &'static str,
    /// The `pairs(t)` call in the `for` header.
    iter_span: Span,
    /// The loop's first variable (the key), when it is one. A write
    /// `t[k] = v` through the CURRENT key `k` targets a field that provably
    /// exists (the traversal is visiting it), and Lua defines assignment to
    /// an existing field — including `= nil` — during traversal. Exempt.
    key_var: Option<LocalId>,
    /// The exemption dies if the key variable is ever reassigned in the
    /// body: after `k = something`, `t[k]` may be a fresh key.
    key_reassigned: bool,
}

/// A recognized builtin call whose first argument the freeze analysis cares
/// about. `table.clone` needs no case: its result is a fresh unfrozen table,
/// and a fresh local already starts at `FrozenState::No`.
enum SpecialCall<'e> {
    /// `table.freeze(arg)`
    Freeze(&'e hir::Expr),
    /// `setmetatable(arg, mt)`
    SetMeta(&'e hir::Expr),
}

pub struct Builder<'m> {
    /// For the closure bodies this body creates (`ClosureAssign`).
    module: &'m hir::Module,
    /// Locals that got a `Declare`: the only ones `ClosureAssign` is
    /// emitted for, and while empty no closure body is walked at all.
    bare_declared: Vec<LocalId>,
    blocks: Vec<BasicBlock>,
    locals: Vec<LocalInfo>,
    scopes: Vec<HashMap<String, LocalId>>,
    current: BlockId,
    fall_off: BlockId,
    loops: Vec<LoopCtx>,
    /// The `pairs`/`ipairs` loops the walk is currently inside (R0111).
    pairs_stack: Vec<PairsFrame>,
    discarded_pcalls: Vec<DiscardedPcall>,
    pairs_mutations: Vec<PairsMutation>,
    shadows: Vec<Shadow>,
    bare_returns: Vec<(BlockId, Span)>,
    /// Params occupy LocalIds `0..param_count` (declared first). Freezing a
    /// parameter is out of scope for this milestone — plan §5 puts it in the
    /// signature — so freeze events are suppressed for these ids.
    param_count: usize,
    /// Locals proven non-nil by the short-circuit structure LEXICALLY
    /// enclosing the expression being walked: the rhs of `x and ...` /
    /// `not x or ...`, an if-expression arm under its condition. A
    /// `NonNilUse` is suppressed for these — the operand cannot evaluate
    /// with the local nil. Purely lexical, so no flow analysis can
    /// invalidate it between the check and the use.
    assume_stack: Vec<Vec<LocalId>>,
    /// Positive while walking a sub-expression that evaluates only on some
    /// paths (short-circuit rhs, if-expression arms): non-nil uses emitted
    /// there are `ConditionalUse`, not `NonNilUse`.
    conditional_depth: u32,
}

impl<'m> Builder<'m> {
    pub fn build(module: &'m hir::Module, body: &hir::Body) -> Cfg {
        let mut b = Builder {
            module,
            bare_declared: Vec::new(),
            blocks: vec![BasicBlock::default(), BasicBlock::default()],
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            current: 0,
            fall_off: 1,
            loops: Vec::new(),
            pairs_stack: Vec::new(),
            discarded_pcalls: Vec::new(),
            pairs_mutations: Vec::new(),
            shadows: Vec::new(),
            bare_returns: Vec::new(),
            param_count: body.params.len(),
            assume_stack: Vec::new(),
            conditional_depth: 0,
        };
        for p in &body.params {
            let id = b.declare(p, LocalKind::Param);
            b.emit(Event::Assign(id, declared_nil(p, ValueNil::Unknown), None));
        }
        let falls = b.walk_block(&body.block);
        let end_fall_block = if falls {
            b.edge(b.current, b.fall_off);
            Some(b.current)
        } else {
            None
        };
        Cfg {
            blocks: b.blocks,
            entry: 0,
            fall_off: b.fall_off,
            locals: b.locals,
            discarded_pcalls: b.discarded_pcalls,
            pairs_mutations: b.pairs_mutations,
            shadows: b.shadows,
            bare_returns: b.bare_returns,
            end_fall_block,
        }
    }

    // --- plumbing ----------------------------------------------------------

    fn new_block(&mut self) -> BlockId {
        self.blocks.push(BasicBlock::default());
        self.blocks.len() - 1
    }

    fn edge(&mut self, from: BlockId, to: BlockId) {
        self.blocks[from].succs.push(to);
    }

    fn emit(&mut self, ev: Event) {
        self.blocks[self.current].events.push(ev);
    }

    fn declare(&mut self, decl: &hir::LocalDecl, kind: LocalKind) -> LocalId {
        let id = self.locals.len();
        // Shadowing (R0011): the name is still visible from an enclosing
        // scope of this body. `_`-prefixed names are deliberate discards.
        if !decl.name.starts_with('_') {
            if let Some(prev) = self.resolve(&decl.name) {
                self.shadows.push(Shadow { local: id, prev });
            }
        }
        self.locals.push(LocalInfo {
            name: decl.name.clone(),
            decl_span: decl.span.clone(),
            kind,
            nilable: decl.ty.as_ref().is_some_and(|t| t.admits_nil),
            annotated: decl.ty.is_some(),
        });
        self.scopes
            .last_mut()
            .expect("scope stack never empty")
            .insert(decl.name.clone(), id);
        id
    }

    fn resolve(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Emit a `NonNilUse` unless the enclosing short-circuit structure
    /// already proves the local non-nil at this spot.
    fn non_nil_use(&mut self, id: LocalId, span: &Span, kind: UseKind) {
        if self.assume_stack.iter().any(|v| v.contains(&id)) {
            return;
        }
        if self.conditional_depth > 0 {
            self.emit(Event::ConditionalUse(id, span.clone(), kind));
        } else {
            self.emit(Event::NonNilUse(id, span.clone(), kind));
        }
    }

    // --- statements ---------------------------------------------------------

    /// Walks a block in the current CFG block. Returns whether control can
    /// fall out of the bottom (false when it ends in return/break/continue).
    fn walk_block(&mut self, block: &hir::Block) -> bool {
        for stmt in &block.stmts {
            self.walk_stmt(stmt);
        }
        match &block.terminator {
            None => true,
            Some(hir::Terminator::Return { exprs, span }) => {
                for e in exprs {
                    self.expr_reads(e);
                }
                if exprs.is_empty() {
                    // A bare `return` hands the caller no values — against a
                    // non-nil return annotation that is the violation itself.
                    // It still edges into fall_off so reachability stays one
                    // question, but the site is recorded so the diagnostic
                    // points HERE, not at the function's `end`.
                    self.bare_returns.push((self.current, span.clone()));
                    self.edge(self.current, self.fall_off);
                }
                false
            }
            Some(hir::Terminator::Break) => {
                if let Some(ctx) = self.loops.last() {
                    let t = ctx.break_target;
                    self.edge(self.current, t);
                }
                false
            }
            Some(hir::Terminator::Continue) => {
                if let Some(ctx) = self.loops.last() {
                    let t = ctx.continue_target;
                    self.edge(self.current, t);
                }
                false
            }
        }
    }

    fn scoped_walk(&mut self, block: &hir::Block) -> bool {
        self.scopes.push(HashMap::new());
        let falls = self.walk_block(block);
        self.scopes.pop();
        falls
    }

    fn walk_stmt(&mut self, stmt: &hir::Stmt) {
        use hir::StmtKind as K;
        if self.blocks[self.current].first_span.is_none() {
            self.blocks[self.current].first_span = Some(stmt.span.clone());
        }
        match &stmt.kind {
            K::Local { decls, init } => {
                for e in init {
                    self.expr_reads(e);
                }
                // Initializer-position `table.freeze(x)` / `setmetatable(x, ..)`:
                // the argument is retyped BEFORE the new names bind, so
                // `local t = table.freeze(t)` freezes the outer `t`.
                for e in init {
                    self.special_call_events(e, &stmt.span);
                }
                let has_value = !init.is_empty();
                let mut decl_ids = Vec::with_capacity(decls.len());
                for (i, decl) in decls.iter().enumerate() {
                    let id = self.declare(decl, LocalKind::Plain);
                    decl_ids.push(id);
                    // Reading an unassigned local yields nil, which is only a
                    // type error when the declared type excludes nil. So
                    // Declare (checkable) is emitted only for a local with an
                    // annotation that does not admit nil; an annotated-`T?`
                    // local holds nil legally from birth, and an unannotated
                    // one is inference territory — no claim yet either way.
                    let checkable = decl.ty.as_ref().is_some_and(|t| !t.admits_nil);
                    if has_value || !checkable {
                        // No initializer list: the binding holds nil, whatever
                        // the annotation says (`local x: any` included). With
                        // one, a name past its end takes a multi-return tail
                        // = unknowable.
                        let vn = if has_value {
                            init.get(i)
                                .map(|e| self.value_nil(e, &[]))
                                .unwrap_or(ValueNil::Unknown)
                        } else {
                            ValueNil::Nil
                        };
                        let vn = declared_nil(decl, vn);
                        // With an initializer list, every name stores a value
                        // (names past the list take the multi-return tail);
                        // a bare `local x` stores nothing worth flagging.
                        let store = has_value.then(|| decl.span.clone());
                        self.emit(Event::Assign(id, vn, store));
                    } else {
                        self.emit(Event::Declare(id));
                        self.bare_declared.push(id);
                    }
                }
                // `local f = table.freeze(t)` freezes BOTH: `t` above (same
                // object), and the new binding `f` here — freeze returns its
                // argument, so whatever the argument was, the RESULT is
                // frozen. Positional pairing only: `local a, b = f()` arity
                // is already a documented milestone shortcut.
                for (&id, e) in decl_ids.iter().zip(init) {
                    if matches!(self.special_call(e), Some(SpecialCall::Freeze(_))) {
                        self.emit(Event::Freeze(id, stmt.span.clone()));
                    }
                }
            }
            K::Assign { targets, values } => {
                for e in values {
                    self.expr_reads(e);
                }
                // Value classification pairs positionally, and only when the
                // counts match — otherwise a multi-return tail shifts the
                // pairing and nothing is knowable.
                let paired = targets.len() == values.len();
                for (i, t) in targets.iter().enumerate() {
                    let vn = if paired {
                        self.value_nil(&values[i], &[])
                    } else {
                        ValueNil::Unknown
                    };
                    self.target_write(t, vn);
                    self.projection_table_write(t, &stmt.span);
                }
            }
            K::CompoundAssign { target, value } => {
                // `x += e` both reads and writes x — and the read is an
                // operand of an operator that throws on nil.
                self.expr_reads(value);
                self.target_read(target);
                if let hir::Target::Name { name, span } = target {
                    if let Some(id) = self.resolve(name) {
                        let span = span.clone();
                        self.non_nil_use(id, &span, UseKind::Operand);
                    }
                }
                // The result of a compound op is never nil (it would have
                // thrown instead).
                self.target_write(target, ValueNil::NonNil);
                self.projection_table_write(target, &stmt.span);
            }
            K::Call(expr) => {
                self.expr_reads(expr);
                self.special_call_events(expr, &stmt.span);
                // R0110: `pcall`/`xpcall` in statement position — the ok
                // flag is discarded, so a failure inside vanishes silently.
                if let Some(callee) = self.discarded_pcall(expr) {
                    self.discarded_pcalls.push(DiscardedPcall {
                        callee,
                        span: stmt.span.clone(),
                    });
                }
                if self.diverges(expr) {
                    // `error(...)` never returns: statements after it live in
                    // a fresh block with no incoming edge (= unreachable).
                    self.current = self.new_block();
                }
            }
            K::Do(block) => {
                if !self.scoped_walk(block) {
                    // The do-block exits (return/break/continue): anything
                    // after it is unreachable, so it goes in a fresh block
                    // with no incoming edge.
                    self.current = self.new_block();
                }
            }
            K::If { cond, then, else_ifs, or_else } => {
                self.walk_if(cond, then, else_ifs, or_else.as_ref());
            }
            K::While { cond, body } => {
                let head = self.new_block();
                self.edge(self.current, head);
                self.current = head;
                self.expr_reads(cond);
                self.emit_nil_checks(cond, false);
                let (pos, neg) = self.cond_facts(cond);

                let after = self.new_block();
                let body_block = self.new_block();
                self.edge(head, body_block);
                if always_truthy(cond) {
                    // `while true do`: the condition never ends the loop, so
                    // there is no exit edge. `after` is reached by `break`
                    // alone, and is unreachable when the body has none.
                } else if neg.is_empty() {
                    self.edge(head, after);
                } else {
                    // The `while x == nil do ... end` idiom: leaving the loop
                    // proves the condition false. `after` can also be reached
                    // by `break`, so the facts go in a pass-through block on
                    // the exit edge alone.
                    let pass = self.new_block();
                    self.edge(head, pass);
                    self.current = pass;
                    for &id in &neg {
                        self.emit(Event::Assume(id));
                    }
                    self.edge(pass, after);
                }

                self.loops.push(LoopCtx { continue_target: head, break_target: after });
                self.current = body_block;
                for &id in &pos {
                    self.emit(Event::Assume(id));
                }
                if self.scoped_walk(body) {
                    self.edge(self.current, head);
                }
                self.loops.pop();
                self.current = after;
            }
            K::Repeat { body, until } => {
                let body_start = self.new_block();
                self.edge(self.current, body_start);
                let latch = self.new_block(); // holds the `until` condition
                let after = self.new_block();

                self.loops.push(LoopCtx { continue_target: latch, break_target: after });
                self.current = body_start;
                // The until-condition sees locals declared in the body, so the
                // scope stays open across both.
                self.scopes.push(HashMap::new());
                if self.walk_block(body) {
                    self.edge(self.current, latch);
                }
                self.current = latch;
                self.expr_reads(until);
                self.scopes.pop();
                self.loops.pop();

                self.edge(latch, body_start);
                // `until false` never ends the loop: only `break` leaves it.
                if !always_falsy(until) {
                    self.edge(latch, after);
                }
                self.current = after;
            }
            K::NumericFor { var, start, end, step, body } => {
                self.expr_reads(start);
                self.expr_reads(end);
                if let Some(step) = step {
                    self.expr_reads(step);
                }
                let head = self.new_block();
                self.edge(self.current, head);
                let after = self.new_block();
                let body_block = self.new_block();
                self.edge(head, body_block);
                self.edge(head, after);

                self.scopes.push(HashMap::new());
                self.current = head;
                let id = self.declare(var, LocalKind::Loop);
                // A numeric-for variable is always a number.
                self.emit(Event::Assign(id, ValueNil::NonNil, None));

                self.loops.push(LoopCtx { continue_target: head, break_target: after });
                self.current = body_block;
                if self.walk_block(body) {
                    self.edge(self.current, head);
                }
                self.loops.pop();
                self.scopes.pop();
                self.current = after;
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.expr_reads(e);
                }
                // R0111 setup: `for ... in pairs(t)` on a plain local.
                // Resolved HERE, before the loop scope opens, so the frame
                // holds the outer `t` — a re-declared `t` inside the body is
                // a different LocalId and never matches.
                let iterated = self.pairs_iterated_local(exprs);

                let head = self.new_block();
                self.edge(self.current, head);
                let after = self.new_block();
                let body_block = self.new_block();
                self.edge(head, body_block);
                self.edge(head, after);

                self.scopes.push(HashMap::new());
                self.current = head;
                let mut var_ids = Vec::with_capacity(vars.len());
                for var in vars {
                    let id = self.declare(var, LocalKind::Loop);
                    var_ids.push(id);
                    self.emit(Event::Assign(id, ValueNil::Unknown, None));
                }
                let tracked = if let Some((local, iter_fn, iter_span)) = iterated {
                    self.pairs_stack.push(PairsFrame {
                        local,
                        iter_fn,
                        iter_span,
                        key_var: var_ids.first().copied(),
                        key_reassigned: false,
                    });
                    true
                } else {
                    false
                };

                self.loops.push(LoopCtx { continue_target: head, break_target: after });
                self.current = body_block;
                if self.walk_block(body) {
                    self.edge(self.current, head);
                }
                self.loops.pop();
                if tracked {
                    self.pairs_stack.pop();
                }
                self.scopes.pop();
                self.current = after;
            }
            K::LocalFunction { decl, body } => {
                // The name is bound (and assigned) before the body parses, so
                // recursion works. The body is a separate arena entry the
                // caller checks independently.
                let id = self.declare(decl, LocalKind::Function);
                self.emit(Event::Assign(id, ValueNil::NonNil, Some(decl.span.clone())));
                self.closure_created(*body);
            }
            K::FunctionDecl { root, assigns_root, body, .. } => {
                // `function foo()` assigns to `foo` if it is a plain local;
                // `function a.b.c()` is a table write we do not model yet.
                if *assigns_root {
                    if let Some(id) = self.resolve(root) {
                        self.emit(Event::Assign(id, ValueNil::NonNil, None));
                    }
                }
                self.closure_created(*body);
            }
        }
    }

    fn walk_if(
        &mut self,
        cond: &hir::Expr,
        then: &hir::Block,
        else_ifs: &[(hir::Expr, hir::Block)],
        or_else: Option<&hir::Block>,
    ) {
        let join = self.new_block();
        self.expr_reads(cond);
        self.emit_nil_checks(cond, false);
        let (pos, mut prev_neg) = self.cond_facts(cond);

        let cond_block = self.current;
        let then_block = self.new_block();
        self.edge(cond_block, then_block);
        self.current = then_block;
        // then_block has exactly one predecessor (the condition), so the
        // condition's positive facts hold on every path through it.
        for &id in &pos {
            self.emit(Event::Assume(id));
        }
        if self.scoped_walk(then) {
            self.edge(self.current, join);
        }

        let mut prev_cond = cond_block;
        for (ei_cond, ei_block) in else_ifs {
            let elseif_cond = self.new_block();
            self.edge(prev_cond, elseif_cond);
            self.current = elseif_cond;
            // Reaching this condition means the previous one was false: its
            // negative facts hold here and on everything downstream.
            for &id in &prev_neg {
                self.emit(Event::Assume(id));
            }
            self.expr_reads(ei_cond);
            self.emit_nil_checks(ei_cond, false);
            let (ei_pos, ei_neg) = self.cond_facts(ei_cond);

            let arm = self.new_block();
            self.edge(elseif_cond, arm);
            self.current = arm;
            for &id in &ei_pos {
                self.emit(Event::Assume(id));
            }
            if self.scoped_walk(ei_block) {
                self.edge(self.current, join);
            }
            prev_cond = elseif_cond;
            prev_neg = ei_neg;
        }

        if let Some(else_block) = or_else {
            let arm = self.new_block();
            self.edge(prev_cond, arm);
            self.current = arm;
            for &id in &prev_neg {
                self.emit(Event::Assume(id));
            }
            if self.scoped_walk(else_block) {
                self.edge(self.current, join);
            }
        } else if prev_neg.is_empty() {
            // No else: the last condition can fail straight through.
            self.edge(prev_cond, join);
        } else {
            // No else, but the failing condition proves facts (the
            // guard-clause idiom: `if x == nil then return end`). The join
            // block has other predecessors, so the facts go in a dedicated
            // pass-through block on this one edge.
            let pass = self.new_block();
            self.edge(prev_cond, pass);
            self.current = pass;
            for &id in &prev_neg {
                self.emit(Event::Assume(id));
            }
            self.edge(pass, join);
        }

        self.current = join;
    }

    // --- narrowing facts from conditions ------------------------------------

    /// The locals a condition proves non-nil: `.0` when the condition is
    /// true, `.1` when it is false. Sound by construction — a local appears
    /// only when the condition's truth value IMPLIES it is not nil:
    ///
    ///   x            -> true: x        (truthy excludes nil)
    ///   x ~= nil     -> true: x        (either operand order)
    ///   x == nil     -> false: x
    ///   type(x) == "nil"  -> false: x
    ///   type(x) == "t"    -> true: x   (any literal other than "nil")
    ///   type(x) ~= "nil"  -> true: x
    ///   not e        -> facts of e, swapped
    ///   a and b      -> true: union   (both held), false: intersection
    ///   a or b       -> true: intersection (either held), false: union
    ///
    /// Everything else contributes nothing — silence, never a wrong fact.
    fn cond_facts(&self, cond: &hir::Expr) -> (Vec<LocalId>, Vec<LocalId>) {
        use hir::Expr as E;
        match cond {
            E::Not(inner) => {
                let (pos, neg) = self.cond_facts(inner);
                (neg, pos)
            }
            // `-e` / `#e` evaluate their operand or throw — either branch of
            // the condition implies the operand's facts held. Same behavior
            // as the pre-UnOp lowering, which read through to the operand.
            E::UnOp { operand, .. } => self.cond_facts(operand),
            E::BinOp { op: hir::BinOp::And, lhs, rhs } => {
                let (pa, na) = self.cond_facts(lhs);
                let (pb, nb) = self.cond_facts(rhs);
                (union(pa, pb), intersect(na, nb))
            }
            E::BinOp { op: hir::BinOp::Or, lhs, rhs } => {
                let (pa, na) = self.cond_facts(lhs);
                let (pb, nb) = self.cond_facts(rhs);
                (intersect(pa, pb), union(na, nb))
            }
            _ => match self.nil_test_leaf(cond) {
                Some(NilTest { id, proves: Proves::WhenTrue, .. }) => (vec![id], Vec::new()),
                Some(NilTest { id, proves: Proves::WhenFalse, .. }) => (Vec::new(), vec![id]),
                None => (Vec::new(), Vec::new()),
            },
        }
    }

    /// The leaf shapes of a nil test — shared by `cond_facts` (which truth
    /// value proves what) and `nil_test_sites` (where the test is):
    ///
    ///   x                 -> non-nil when true   (truthy excludes nil)
    ///   x ~= nil          -> when true           (either operand order)
    ///   x == nil          -> when false
    ///   type(x) == "nil"  -> when false
    ///   type(x) == "t"    -> when true           (any literal other than "nil")
    ///   type(x) ~= "nil"  -> when true
    ///   type(x) ~= "t"    -> when false          (proves nothing when true — x
    ///                                             could be nil — but its FALSE
    ///                                             branch means the type IS `t`:
    ///                                             the negated guard-clause spelling)
    ///
    /// Span is the tested name's occurrence. Everything else is `None` —
    /// silence, never a wrong fact.
    fn nil_test_leaf(&self, cond: &hir::Expr) -> Option<NilTest> {
        use hir::Expr as E;
        match cond {
            E::Name { name, span } => self.resolve(name).map(|id| NilTest {
                id,
                span: span.clone(),
                proves: Proves::WhenTrue,
                pure: true,
            }),
            E::BinOp { op: op @ (hir::BinOp::Eq | hir::BinOp::Ne), lhs, rhs } => {
                let eq = *op == hir::BinOp::Eq;
                for (a, b) in [(&**lhs, &**rhs), (&**rhs, &**lhs)] {
                    // `x == nil` / `nil == x`.
                    if let (E::Name { name, span }, E::Literal { prim: hir::Prim::Nil, .. }) =
                        (a, b)
                    {
                        if let Some(id) = self.resolve(name) {
                            let proves = if eq { Proves::WhenFalse } else { Proves::WhenTrue };
                            return Some(NilTest { id, span: span.clone(), proves, pure: true });
                        }
                    }
                    // `type(x) == "literal"` / `"literal" == type(x)`.
                    if let (
                        Some((id, span)),
                        E::Literal { prim: hir::Prim::String, text: Some(t), .. },
                    ) = (self.type_call_arg(a), b)
                    {
                        let names_nil = t == "nil";
                        let proves = match (eq, names_nil) {
                            (true, true) => Proves::WhenFalse,
                            (true, false) => Proves::WhenTrue,
                            (false, true) => Proves::WhenTrue,
                            (false, false) => Proves::WhenFalse,
                        };
                        return Some(NilTest { id, span, proves, pure: names_nil });
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Every local a CONDITION nil-tests (pure tests only), with the tested
    /// name's span — the R0202 check sites. Same combinators as
    /// `cond_facts`; polarity is irrelevant here, a test is a test. In
    /// condition position both operands of `and`/`or` decide the branch,
    /// so both sides are tests. `guards_only` restricts the walk to the
    /// left operands — expression position, where `flag and item or
    /// fallback` SELECTS `item` rather than testing it (see `expr_reads`).
    fn nil_test_sites(&self, cond: &hir::Expr, guards_only: bool, out: &mut Vec<(LocalId, Span)>) {
        use hir::Expr as E;
        match cond {
            E::Not(inner) | E::UnOp { operand: inner, .. } => {
                self.nil_test_sites(inner, guards_only, out)
            }
            E::BinOp { op: hir::BinOp::And | hir::BinOp::Or, lhs, rhs } => {
                self.nil_test_sites(lhs, guards_only, out);
                if !guards_only {
                    self.nil_test_sites(rhs, guards_only, out);
                }
            }
            _ => {
                if let Some(NilTest { id, span, pure: true, .. }) = self.nil_test_leaf(cond) {
                    out.push((id, span));
                }
            }
        }
    }

    /// Emit a `NilCheck` in the current block for each local `cond` tests.
    /// An `and`/`or` inside an `if` condition is reached twice (from the
    /// expression walk and from the statement); R0202 dedups by site.
    fn emit_nil_checks(&mut self, cond: &hir::Expr, guards_only: bool) {
        let mut sites = Vec::new();
        self.nil_test_sites(cond, guards_only, &mut sites);
        for (id, span) in sites {
            self.emit(Event::NilCheck(id, span));
        }
    }

    /// Syntactic nil-ness of an assigned value. `assumed` carries locals an
    /// enclosing if-expression arm's condition has proven non-nil — what
    /// makes the default-fill idiom (`x = if type(x) == "number" then x
    /// else 1`) classify. Sound direction: `Unknown` is always safe.
    fn value_nil(&self, e: &hir::Expr, assumed: &[LocalId]) -> ValueNil {
        use hir::Expr as E;
        if matches!(e, E::Literal { prim: hir::Prim::Nil, .. }) {
            return ValueNil::Nil;
        }
        let non_nil = match e {
            // `nil` already returned above, so every literal left is a value.
            E::Literal { .. } => true,
            // Constructors and closures are values; `not e` is a boolean.
            E::Table(_) | E::Function(_) | E::Not(_) => true,
            // Transparent, matching the pre-UnOp read-through lowering.
            // (`-e`/`#e` are in fact never nil — upgrading this to `true` is
            // a deliberate later precision change, not a drive-by.)
            E::UnOp { operand, .. } => {
                self.value_nil(operand, assumed) == ValueNil::NonNil
            }
            E::BinOp { op, lhs, rhs } => match op {
                // `a or b` is nil only when b can be (a is picked only when
                // truthy, and truthy excludes nil).
                hir::BinOp::Or => self.value_nil(rhs, assumed) == ValueNil::NonNil,
                // `a and b` can produce a falsy `a` — nil included.
                hir::BinOp::And => {
                    self.value_nil(lhs, assumed) == ValueNil::NonNil
                        && self.value_nil(rhs, assumed) == ValueNil::NonNil
                }
                // Arithmetic/concat/comparison: a nil operand throws before
                // the assignment happens, and the result is never nil.
                _ => true,
            },
            E::Name { name, .. } => self
                .resolve(name)
                .is_some_and(|id| assumed.contains(&id)),
            E::IfExpr(arms) => arms.iter().all(|(cond, val)| {
                let ext = match cond {
                    Some(c) => union(assumed.to_vec(), self.cond_facts(c).0),
                    None => assumed.to_vec(),
                };
                self.value_nil(val, &ext) == ValueNil::NonNil
            }),
            _ => false,
        };
        if non_nil { ValueNil::NonNil } else { ValueNil::Unknown }
    }

    /// `type(x)` / `typeof(x)` on the unshadowed global, argument a plain
    /// resolvable local — the LocalId of x.
    fn type_call_arg(&self, expr: &hir::Expr) -> Option<(LocalId, Span)> {
        let hir::Expr::Call { base, args, method: None, .. } = expr else {
            return None;
        };
        let hir::Expr::Name { name, .. } = &**base else {
            return None;
        };
        if (name != "type" && name != "typeof") || self.resolve(name).is_some() {
            return None;
        }
        let [hir::Expr::Name { name: arg, span }] = args.as_slice() else {
            return None;
        };
        self.resolve(arg).map(|id| (id, span.clone()))
    }

    // --- freeze / setmetatable recognition ----------------------------------

    /// Recognize a call whose argument the freeze analysis retypes, in
    /// statement or initializer position only (plan §5: inference is
    /// restricted to fresh allocations and simple locals; a freeze buried in
    /// an arbitrary expression is not recognized — a documented milestone
    /// limit, not a decision).
    fn special_call<'e>(&self, expr: &'e hir::Expr) -> Option<SpecialCall<'e>> {
        let hir::Expr::Call { base, args, .. } = expr else {
            return None;
        };
        let first_arg = args.first()?;
        match &**base {
            // `table.freeze(x)` — only when `table` is the unshadowed global.
            hir::Expr::Index { base: tbl, key: None, name: Some(field), .. } => {
                let hir::Expr::Name { name, .. } = &**tbl else {
                    return None;
                };
                if name == "table" && field == "freeze" && self.resolve("table").is_none() {
                    Some(SpecialCall::Freeze(first_arg))
                } else {
                    None
                }
            }
            // `setmetatable(x, mt)` — unshadowed global.
            hir::Expr::Name { name, .. }
                if name == "setmetatable" && self.resolve("setmetatable").is_none() =>
            {
                Some(SpecialCall::SetMeta(first_arg))
            }
            _ => None,
        }
    }

    /// Emit the freeze/setmetatable events for a statement- or
    /// initializer-position call.
    ///
    /// Only arguments that are plain locals are tracked. Freezing a field,
    /// a global, or a parameter is out of scope for this milestone — no
    /// event, no diagnostic (plan §5: those go in the signature).
    fn special_call_events(&mut self, expr: &hir::Expr, span: &Span) {
        match self.special_call(expr) {
            Some(SpecialCall::Freeze(arg)) => {
                if let Some(id) = self.plain_local(arg) {
                    self.emit(Event::Freeze(id, span.clone()));
                }
            }
            Some(SpecialCall::SetMeta(arg)) => {
                if let Some(id) = self.plain_local(arg) {
                    self.emit(Event::SetMeta(id, span.clone()));
                }
            }
            None => {}
        }
    }

    /// The argument as a resolvable non-parameter local, if it is one.
    fn plain_local(&self, expr: &hir::Expr) -> Option<LocalId> {
        let hir::Expr::Name { name, .. } = expr else {
            return None;
        };
        let id = self.resolve(name)?;
        // Params occupy 0..param_count; freeze-of-parameter is signature
        // territory (plan §5), so it produces no event this milestone.
        (id >= self.param_count).then_some(id)
    }

    /// A write through a single-level projection of a local — `t.x = 1` or
    /// `t[k] = v` — writes the local's own table. Deeper chains
    /// (`t.a.b = 1`) mutate a nested table, which a *shallow* freeze does not
    /// protect, so they are deliberately not events (case 6 of plan §9).
    fn projection_table_write(&mut self, target: &hir::Target, span: &Span) {
        let hir::Target::Projection(e) = target else {
            return;
        };
        let hir::Expr::Index { base, key, .. } = e else {
            return;
        };
        let hir::Expr::Name { name, .. } = &**base else {
            return;
        };
        if let Some(id) = self.resolve(name) {
            self.emit(Event::TableWrite(id, span.clone()));
            self.check_pairs_mutation(id, key.as_deref(), span);
        }
    }

    /// R0111: is this single-level write through `id` happening while `id`
    /// is being iterated by an enclosing `pairs`/`ipairs` loop?
    ///
    /// Exemption: `t[k] = v` where `k` is (still) the loop's own key
    /// variable writes the field the traversal is currently visiting, which
    /// provably exists — and Lua defines assignment to an existing field
    /// during traversal, including `t[k] = nil` removal. Any other key may
    /// be an insertion, which is undefined behavior, and Rime cannot prove
    /// key existence for anything but the current key.
    fn check_pairs_mutation(&mut self, id: LocalId, key: Option<&hir::Expr>, span: &Span) {
        let key_id = match key {
            Some(hir::Expr::Name { name, .. }) => self.resolve(name),
            _ => None,
        };
        // Current-key writes are defined for EVERY live traversal of this
        // table (the field exists), so one matching frame exempts the write.
        let current_key = self.pairs_stack.iter().any(|f| {
            f.local == id && !f.key_reassigned && f.key_var.is_some() && f.key_var == key_id
        });
        if current_key {
            return;
        }
        // Innermost matching frame gets the diagnostic's secondary label.
        if let Some(frame) = self.pairs_stack.iter().rev().find(|f| f.local == id) {
            self.pairs_mutations.push(PairsMutation {
                local: id,
                iter_fn: frame.iter_fn,
                write_span: span.clone(),
                iter_span: frame.iter_span.clone(),
            });
        }
    }

    /// Recognize `for ... in pairs(t)` / `ipairs(t)`: exactly one iterator
    /// expression, calling the unshadowed global, on an argument that is a
    /// plain resolvable local. Anything else — `next, t`, a custom iterator,
    /// `pairs(t.field)` — is out of scope: silence, never a false positive.
    fn pairs_iterated_local(
        &self,
        exprs: &[hir::Expr],
    ) -> Option<(LocalId, &'static str, Span)> {
        let [expr] = exprs else {
            return None;
        };
        let hir::Expr::Call { base, args, method: None, span, .. } = expr else {
            return None;
        };
        let hir::Expr::Name { name, .. } = &**base else {
            return None;
        };
        let iter_fn = match name.as_str() {
            "pairs" => "pairs",
            "ipairs" => "ipairs",
            _ => return None,
        };
        if self.resolve(name).is_some() {
            return None; // shadowed: not the global pairs/ipairs
        }
        let [arg] = args.as_slice() else {
            return None;
        };
        let hir::Expr::Name { name: table_name, .. } = arg else {
            return None;
        };
        let id = self.resolve(table_name)?;
        Some((id, iter_fn, span.clone()))
    }

    // --- expressions --------------------------------------------------------

    /// Statement-position call to the unshadowed global `pcall`/`xpcall`
    /// (R0110). Same recognition shape as `diverges`: a plain-name base,
    /// resolve() misses = unshadowed global.
    fn discarded_pcall(&self, expr: &hir::Expr) -> Option<&'static str> {
        let hir::Expr::Call { base, method: None, .. } = expr else {
            return None;
        };
        let hir::Expr::Name { name, .. } = &**base else {
            return None;
        };
        let callee = match name.as_str() {
            "pcall" => "pcall",
            "xpcall" => "xpcall",
            _ => return None,
        };
        self.resolve(name).is_none().then_some(callee)
    }

    /// Statement-position call that never returns: the unshadowed global
    /// `error(...)`, and the unshadowed global `assert` on a literal `false`
    /// or `nil`. Any other `assert` condition would need constant folding
    /// and keeps its fall-through edge.
    fn diverges(&self, expr: &hir::Expr) -> bool {
        let hir::Expr::Call { base, args, method: None, .. } = expr else {
            return false;
        };
        let hir::Expr::Name { name, .. } = &**base else {
            return false;
        };
        if self.resolve(name).is_some() {
            return false;
        }
        match name.as_str() {
            "error" => true,
            "assert" => args.first().is_some_and(always_falsy),
            _ => false,
        }
    }

    fn expr_reads(&mut self, expr: &hir::Expr) {
        use hir::Expr as E;
        match expr {
            E::Name { name, span } => {
                if let Some(id) = self.resolve(name) {
                    self.emit(Event::Read(id, span.clone()));
                }
            }
            E::Call { base, args, method, span, .. } => {
                self.expr_reads(base);
                for a in args {
                    self.expr_reads(a);
                }
                // The receiver/callee must be non-nil AT the call — checked
                // against the state before this call's own kill applies.
                if let E::Name { name, span: use_span } = peel_unop(base) {
                    if let Some(id) = self.resolve(name) {
                        let kind = if method.is_some() { UseKind::Method } else { UseKind::Call };
                        self.non_nil_use(id, use_span, kind);
                    }
                }
                let kind = call_kind(base, method.as_deref());
                self.emit(Event::AnyCall(span.clone(), kind));
                // `assert(cond, ...)` returns only when `cond` held, so what
                // follows it in this block is guarded like the body of
                // `if cond then`. Only for the unshadowed global, and only
                // where the call runs on every path through its expression
                // — not the rhs of `and`/`or` or an if-expression arm.
                if self.conditional_depth == 0 && method.is_none() {
                    if let (E::Name { name, .. }, Some(cond)) = (&**base, args.first()) {
                        if name == "assert" && self.resolve(name).is_none() {
                            for id in self.cond_facts(cond).0 {
                                self.emit(Event::Asserted(id));
                            }
                        }
                    }
                }
            }
            E::Index { base, key, .. } => {
                self.expr_reads(base);
                if let Some(k) = key {
                    self.expr_reads(k);
                }
                if let E::Name { name, span } = peel_unop(base) {
                    if let Some(id) = self.resolve(name) {
                        self.non_nil_use(id, span, UseKind::Index);
                    }
                }
            }
            E::BinOp { op, lhs, rhs } => {
                self.expr_reads(lhs);
                // Short-circuit: the rhs of `and` runs only when the lhs was
                // truthy, of `or` only when it was falsy — the dominant guard
                // idiom (`x and x.f`, `not x or x.f`) lives here, not in an
                // `if`.
                let guard = match op {
                    hir::BinOp::And => Some(self.cond_facts(lhs).0),
                    hir::BinOp::Or => Some(self.cond_facts(lhs).1),
                    _ => None,
                };
                match guard {
                    Some(facts) => {
                        // The lhs is a nil test of whatever it names, in
                        // expression position or not (R0202 check site) —
                        // but only its guard side: in `flag and item or
                        // fallback`, `item` is the value selected, not a
                        // test, so a nested `and`/`or` contributes its
                        // left operand alone.
                        self.emit_nil_checks(lhs, true);
                        self.assume_stack.push(facts);
                        self.conditional_depth += 1;
                        self.expr_reads(rhs);
                        self.conditional_depth -= 1;
                        self.assume_stack.pop();
                    }
                    None => self.expr_reads(rhs),
                }
                if op.requires_non_nil_operands() {
                    for side in [&**lhs, &**rhs] {
                        // `#t > 0`: the `#` throws on nil t just like the
                        // comparison would — see through unary layers, as
                        // the pre-UnOp lowering did.
                        if let E::Name { name, span } = peel_unop(side) {
                            if let Some(id) = self.resolve(name) {
                                self.non_nil_use(id, span, UseKind::Operand);
                            }
                        }
                    }
                }
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.expr_reads(inner),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.expr_reads(key);
                            self.expr_reads(value);
                        }
                        hir::TableField::Named { value, .. } => self.expr_reads(value),
                        hir::TableField::Item(e) => self.expr_reads(e),
                    }
                }
            }
            E::IfExpr(arms) => {
                // Arm N evaluates only when every earlier condition was
                // false (their negative facts hold) and, for the value, its
                // own condition was true.
                // Only the first condition always evaluates; every later
                // condition and every value is conditional (R0202).
                let mut acc_neg: Vec<LocalId> = Vec::new();
                for (i, (cond, val)) in arms.iter().enumerate() {
                    self.assume_stack.push(acc_neg.clone());
                    match cond {
                        Some(c) => {
                            if i > 0 {
                                self.conditional_depth += 1;
                            }
                            self.expr_reads(c);
                            if i > 0 {
                                self.conditional_depth -= 1;
                            }
                            self.emit_nil_checks(c, false);
                            let (pos, neg) = self.cond_facts(c);
                            self.assume_stack.push(pos);
                            self.conditional_depth += 1;
                            self.expr_reads(val);
                            self.conditional_depth -= 1;
                            self.assume_stack.pop();
                            acc_neg = union(acc_neg, neg);
                        }
                        None => {
                            self.conditional_depth += 1;
                            self.expr_reads(val);
                            self.conditional_depth -= 1;
                        }
                    }
                    self.assume_stack.pop();
                }
            }
            E::Composite(parts) => {
                for p in parts {
                    self.expr_reads(p);
                }
            }
            E::Function(id) => self.closure_created(*id),
            E::Literal { .. } | E::Leaf => {}
        }
    }

    /// A closure over this body's scope comes into existence here. Emits
    /// `ClosureAssign` for each local declared without a value that the
    /// closure assigns — resolved at the creation site, so a same-named
    /// local the closure cannot see is not touched.
    fn closure_created(&mut self, id: hir::BodyId) {
        if self.bare_declared.is_empty() {
            return;
        }
        for name in capture::closure_writes(self.module, id) {
            if let Some(local) = self.resolve(&name) {
                if self.bare_declared.contains(&local) {
                    self.emit(Event::ClosureAssign(local));
                }
            }
        }
    }

    fn target_read(&mut self, target: &hir::Target) {
        match target {
            hir::Target::Name { name, span } => {
                if let Some(id) = self.resolve(name) {
                    self.emit(Event::Read(id, span.clone()));
                }
            }
            hir::Target::Projection(e) => self.expr_reads(e),
        }
    }

    /// A write to an assignment target. Only plain local names count as
    /// assignments; `t.x = v` mutates the table, it does not (re)define `t` —
    /// but it does READ `t`.
    fn target_write(&mut self, target: &hir::Target, vn: ValueNil) {
        match target {
            hir::Target::Name { name, span } => {
                if let Some(id) = self.resolve(name) {
                    self.emit(Event::Assign(id, vn, Some(span.clone())));
                    // Reassigning a pairs-loop key variable kills the
                    // current-key exemption for that frame (R0111): after
                    // `k = something`, `t[k]` may name a fresh key.
                    for frame in &mut self.pairs_stack {
                        if frame.key_var == Some(id) {
                            frame.key_reassigned = true;
                        }
                    }
                }
                // Unresolved = global write; rules::globals (R0023) owns
                // that via its own module-level walk.
            }
            hir::Target::Projection(e) => self.expr_reads(e),
        }
    }
}
