# Rime — Design Notes & Session Plan

> This document describes the design Rime is heading for; [`README.md`](README.md) says what the current release checks.

A strict-dialect static checker for Luau, targeting Roblox.

Working doc. Captures what we've settled, what's still open, and the plan to
resolve the open questions by writing a test corpus first.

---

## 1. What this is

Rime reads ordinary `.luau` files, applies a far stricter set of rules than
Roblox's analyzer, and either accepts or rejects. Not a compiler. Not a
transpiler. No new syntax.

### Where the idea comes from

The goal is the kind of static assurance you get from a **C++ or C# toolchain** —
if it compiles, an entire category of failure is gone — brought to a language
that currently offers nothing like it.

Worth being precise about which parts of that we're actually borrowing:

- **From C++: const-correctness.** This is the deepest parallel and it shaped
  §5. C++ distinguishes `const T*` (pointee immutable) from `T* const` (pointer
  immutable) — exactly our split between value immutability and binding
  immutability. `const T&` as a parameter type is `Readonly<T>`: the callee may
  read, may not write, and the caller keeps ownership. C++ enforces it at compile
  time with no runtime cost, which is the same trade we're making.
- **From C++: no implicit escape hatch.** Casting away const requires
  `const_cast`, which is deliberately verbose and greppable. That's the model for
  `--!trusted` (§3).
- **From C#: nullable reference types and definite assignment.** `string?` vs
  `string`, with flow analysis proving a value non-null before use — that's our
  nil-safety, and C# 8's rollout is a good case study in how a strictness
  migration lands on an existing ecosystem.
- **From C#: exhaustiveness on switch expressions.** Directly our tagged-union
  requirement.
- **From Java/C#: definite assignment and definite return.** Reading a local
  before every path assigns it, or falling off the end of a function that
  declares a return type. Both produce a silent `nil` in Luau, which is exactly
  our theorem's territory. See §3.
- **From Rust `#[must_use]` / C++ `[[nodiscard]]`:** ignoring a returned value
  that carries failure information. `pcall` is the case that matters.

### The honest limitation

**Rust's borrow checker is the machinery that would properly answer §7.1 and
§7.2, and we are not building it.** Ownership, lifetimes, and aliasing-XOR-mutation
are the real solution to "can this value change between the check and the use."
We're buying a cheap approximation instead — hoist to a local, no narrowing
across yields, frozen values exempt. That's a deliberate trade for a
weekend-scale core over a months-scale one, but it should be named as an
approximation rather than presented as equivalent.

Worth noting the inverse, though: Rust's `Send`/`Sync` data-race freedom is moot
here (scripts are single-threaded per VM), and **our yield-effect tracking is the
moral equivalent** — catching interleaving hazards rather than true races. That's
the framing for the README.

What we are explicitly **not** borrowing is C++'s diagnostics, which are among
the worst in mainstream tooling — a one-character mistake in a templated call can
produce hundreds of lines of instantiation backtrace. Rime's target is
**C++-grade rigour with Rust/Elm-grade error messages.** See §6; this is a
first-class goal, not polish.

### The claim

> If a module checks clean, it cannot raise a runtime type error:
> no indexing nil, no calling a non-function, no arithmetic on non-numbers,
> no missing-field access, no non-exhaustive match.

Explicitly **not** covered — say so in the README:

- logic errors
- infinite loops / timeouts
- errors raised by the engine and caught via `pcall`
- anything past an untrusted boundary that wasn't validated (see §4)

An overclaimed "correct" is how people learn to distrust a checker. The theorem
is the product; everything below exists to serve it.

### Non-goals

- **Runtime enforcement.** We don't own the compiler — Roblox compiles source
  server-side. Guarantees are static-only, with the sole exception of
  `table.freeze`, which the VM enforces for free.
- **Type inference as a headline feature.** Signatures are mandatory (§3), so
  checking gets us a long way before inference is needed. Inference is where both
  the complexity and the slowness live.
- **Compatibility with untyped legacy code.** Roblox's analyzer optimizes for "no
  false positives on existing code." We are deliberately reversing that priority.
  Adoption is per-module and opt-in.

### Name propagation

Settle these before writing twenty test cases that need renumbering.

| Surface | Form |
|---|---|
| pragma | `--!rime` |
| diagnostic codes | `R0041` |
| CLI | `rime check`, `rime explain R0041` |
| config | `rime.toml` |
| shim module (`Frozen<T>`, `Readonly<T>`) | `rime-types` |

Check crates.io and GitHub availability — RIME is also an established Chinese
input method engine. Different domain, no real conflict, but it owns search
results.

---

## 2. The subset constraint (hard rule)

**Our dialect is a strict subset of Luau syntax.** Every file we accept is
already valid `.luau`.

Consequences, all good:

- no build step, no source maps, no fighting Rojo
- luau-lsp, StyLua, Selene keep working on our code
- we ship alongside the existing toolchain instead of replacing it

The moment we add one piece of grammar Luau lacks, we own a compiler forever.

Where we need something the language doesn't have, the two legal moves are:

1. **Magic type aliases.** User defines `export type Frozen<T> = T` in the shim
   module. Real Luau sees a trivial generic alias and shrugs; other tools ignore
   it; *Rime* special-cases the name and enforces it.
2. **`--!` pragma comments**, which Luau already ignores.

Anything else is out of bounds.

---

## 3. Settled decisions

### Mandatory annotations

Every function signature and every module export is annotated. Unannotated is an
error, not implicit `any`.

This is the OCaml `.mli` / Rust trick and it buys correctness *and* speed at
once: parallel per-module checking, cheap incrementality keyed on signatures,
non-exponential inference, and far better error messages.

### Banned outright

- `::` casts — or allowed only at declared boundary functions
- `any` in user code
- `getfenv` / `setfenv` / `loadstring`
- implicit globals
- dynamic `require` — statically resolvable paths only
- arbitrary `__index` chains; only the canonical `setmetatable({}, Class)` shape

### Required

- strict nil-safety with flow narrowing (`FindFirstChild` is `Instance?` and you
  must prove it)
- exhaustiveness on tagged unions

### Yields as an effect

**This is the differentiator.** No existing tool catches it, and it's the actual
source of Roblox bugs.

`WaitForChild`, `task.wait`, `:InvokeServer()`, DataStore calls — any of them
suspend, and the world changes underneath you. So:

- track yield as an effect in every function's type
- **kill all narrowing across a yield point** (except for deeply frozen values,
  see §5)
- flag `Instance` handles used after a possible `Destroy`
- statically reject a yield inside a `Changed` / `GetPropertyChangedSignal`
  handler or `BindToRenderStep` callback — currently these just silently
  misbehave

### CFG-derived checks

All of these fall out of the control-flow graph we're already building for yield
tracking. Cheap, sound, real bugs, and — importantly — **none of them touch the
aliasing question blocking §7**, so they can ship first.

- **Definite assignment.** `local x` followed by branchy assignment, then a read
  on a path that didn't assign. Silent `nil` in Luau today.
- **Definite return.** A function annotated `-> number` where a branch falls off
  the end returns `nil`. Same walk.
- **Unreachable code.** Free once the CFG exists. Rust errors on this; we should
  at least warn.
- **Must-use values.** Ignoring the result of `pcall` silently swallows errors —
  one of the most common real Roblox bugs. Small annotation, big win.
- **Mutation during `pairs`.** Adding or removing keys while iterating is
  undefined behavior in Lua. The direct analogue of C++ iterator invalidation.
- **Definite field assignment in constructors.** C# requires every `readonly`
  field assigned before the constructor returns. Our canonical
  `setmetatable({}, Class)` shape has the identical hazard: a field declared in
  the type but never set in the constructor is `nil` at first read.

### Aliasing (provisional — see §7)

Tables are mutable and shared, so narrowing a field is unsound if anything can
write it. Current rule: **only narrow `local` bindings that aren't captured by
any closure.** Make people write `local x = t.x` first. Rejects some valid code
but explains in one sentence.

### The escape hatch

One hatch, and it should be **ugly**: a `--!trusted` pragma on a decoder function
whose return type we take on faith. Greppable, countable in CI, auditable in
review. `unsafe` and `const_cast` both work because they're rare and visible.

---

## 4. Untyped boundaries

Static analysis fundamentally cannot close these, and we can't insert runtime
checks:

- `OnServerEvent` -> `...any` from a possibly hostile client
- DataStore reads
- `HttpService:JSONDecode`
- attributes, `StringValue`s
- the `Instance` tree via `WaitForChild`

**Design:** these are *declared* boundaries. The checker forces a call to a
decoder/validator — a Zod-ish library the user writes in ordinary Luau — which
returns a typed value or fails.

The guarantee becomes **"sound given validated boundaries."** Same claim Rust
makes about `unsafe`. Real and defensible.

---

## 5. Const / immutability

Three distinct things that all get called const:

| | what it means | C++ analogue | how we get it |
|---|---|---|---|
| **1. Binding immutability** | never reassigned | `T* const` | infer silently — it's the narrowing machinery we already need |
| **2. Value immutability** | table can't be mutated | `const T*` / `const T&` | `table.freeze` + `Frozen<T>` |
| **3. Compile-time constants** | singleton types | `constexpr` | Luau already has these; needed for exhaustiveness anyway |

**#2 is the one that pays.** `table.freeze` is the only runtime enforcement
Roblox gives us for free, and `table.isfrozen` is a queryable predicate. Note
this is a guarantee C++ `const` doesn't actually have — it's advisory and
castable-away, whereas a frozen Luau table throws.

### Why frozen matters: it survives yields

Killing narrowing at every suspension point is the rule users will hate most. But
it's only necessary because the world can mutate underneath you. A **deeply
frozen** value can't, so narrowing on it is sound across `task.wait`,
`WaitForChild`, everything.

Const stops being nice-to-have and becomes the escape valve that makes the yield
rule tolerable — and it pushes users toward code that's actually safer. Same
trick kills the aliasing hazard: a frozen table has no aliasing by construction.

### The type lattice

```
Frozen<T>  <:  Readonly<T>
```

Neither flows back to `T`. Collapse `Frozen<Frozen<T>>` or inference through
generic helpers will stack them.

**Deliberate divergence from real Luau:** `local t: {a: number} = table.freeze({a = 1})`
is legal Luau and an **error** in Rime. Correct behavior; needs a good message
pointing at `Frozen<{a: number}>`.

`Readonly` is *not* transitive by default — nested tables need their own
annotation. (C++ has the same shallow-const property, and it bites people the
same way.) `DeepFrozen` = shallow-frozen **and** every value type is primitive or
itself `DeepFrozen`. This is computable from the type, not from flow.

### Inferring frozen

Frozen is a property of the **object**, not the binding, and the VM enforces it.
So we don't have to prove absence of mutation after the freeze point — mutation
is impossible, it throws. The analysis reduces to: *which object got frozen?*
That's identity, not effects.

**Infer only for fresh allocations:** `local X = table.freeze({...})` on a table
literal, or a local provably unaliased between allocation and freeze. That's the
idiom people write, covers nearly all real usage, needs no alias analysis.

Everything else — freezing a parameter, a field, an escaped table — goes in the
signature, which is fine because signatures are mandatory. Keeps it modular:
per-module, parallel, no interprocedural inference.

### The aliasing wrinkle

`table.freeze(t)` returns **the same table**. So the return type only covers the
new binding:

```lua
local t = {}
local f = table.freeze(t)  -- f: Frozen<{}>, but t: {} — same object
t.x = 1                    -- typechecks, throws at runtime  <- violates the theorem
```

Fix: **flow-sensitive retyping** of the argument. After the call, `t` *becomes*
`Frozen<T>` for the rest of the scope. Trivial for a simple local; impossible if
`t` is a field or escaped — another reason to restrict inference to fresh
allocations and reject the rest.

The signature is the ergonomic half; the retyping is the sound half. Both needed.

### Our stdlib defs

Lives in Rime's `.d.luau`, **not** the parser. Parser stays plain-Luau.

```
table.freeze:    <T>(T) -> Frozen<T>        -- real Luau: <T>(T) -> T
table.clone:     <T>(Frozen<T>) -> T
table.isfrozen:  predicate, narrows to Frozen<T>
```

Also worth constraining `T` to table types — `table.freeze(5)` throws at runtime
and real Luau's signature happily accepts it.

`table.isfrozen` narrowing is notable: a rare case where we get **runtime-checked
narrowing without owning the compiler**, because the user writes the check
themselves. Useful at exactly the untyped boundaries where we have nothing else.

### Roblox-specific const gotchas

- `table.freeze` is shallow. Deep vs shallow must be distinct in the type system
  or the yield-narrowing guarantee is false one level down.
- Can't `setmetatable` a frozen table -> class construction must freeze **last**.
  If we narrow through `__index`, the metatable needs freezing too or the
  guarantee leaks one level up.
- **`Instance` properties are never const.** The engine mutates them from C++.
  No amount of const buys narrowing on `part.Parent`.
- Module returns are `require`-cached and shared process-wide. An exported
  mutable table is shared global state -> **exported tables must be frozen**
  (hard rule).

---

## 6. Diagnostics

**A first-class goal, weighted equally with soundness.** A strict checker with
bad errors is worse than no checker: people disable it, or cargo-cult fixes they
don't understand. Rime is by design going to reject a lot of code that *looks*
fine, including a whole bucket of code that genuinely is fine (§8). Every one of
those rejections is a moment where the user decides whether the tool is worth
keeping. The message is the product surface.

Models: **Rust** (spans, labels, structured notes, `--explain`) and **Elm**
(prose, no jargon, treats the user as a colleague rather than a compiler).
Anti-model: **C++ template instantiation backtraces.**

### Rules

1. **Every diagnostic answers four questions**, in this order: what happened,
   where, why it's a problem, how to fix it. If the fix line can't be written,
   the rule may be too vague to ship.
2. **Show source, with spans and labels.** Point at the write site *and* the
   freeze site. Point at the yield *and* the narrowing it killed. Most Rime
   errors are about a relationship between two places in the file — one caret is
   not enough.
3. **Plain language, no type theory.** Never "covariant", "unification failure",
   "supertype". Say "this table can't be changed after `table.freeze` on line 3."
4. **Never print an internal type.** No solver artifacts, no `t1234`, no
   inference variables. If a type can't be rendered as something the user could
   have typed themselves, that's a bug in the renderer.
5. **One error per root cause.** No cascades. A single bad annotation must not
   produce nine downstream errors — suppress dependents and say so.
6. **Suggest the concrete fix.** "Add `table.freeze` here", "hoist to
   `local x = t.x` before the loop", "annotate as `Readonly<{[string]: boolean}>`".
   Where the fix is mechanical, emit it as a structured suggestion so it can
   become an LSP code action later.
7. **Explain the conservative rejections differently.** When we reject valid code
   (§8, `reject-conservative`), say so outright: this is safe today but Rime
   can't prove it stays safe. Name the rule and link the explanation. Users
   forgive a limitation they understand and resent one that pretends to be their
   bug.
8. **`rime explain R0041`** — long-form prose for every code, with a worked
   example and the reasoning. This is where the type theory is allowed to live.
9. **Sort by source order, cap the output.** Errors arrive in reading order.

### Testable

Snapshot the **rendered** diagnostic, not just the code (§8). That makes message
quality a reviewable diff rather than an aspiration, and it means a regression in
wording fails CI like any other regression.

---

## 7. Open decisions

These are what the test corpus is meant to settle. Ordered by how much design
they drag behind them.

### 7.1 Does narrowing survive a function call? <- decide first

This one decides how much aliasing analysis the entire design carries. It's the
difference between a weekend-scale core and a months-scale one. Rust answers it
with the borrow checker; we're picking one of the approximations below (see §1,
"The honest limitation").

- **Conservative:** any call invalidates narrowing on anything reachable. Cheap,
  sound, noisy.
- **Effect-based:** calls declare whether they mutate; only matching narrowings
  die. More precise, needs effect annotations on every signature.
- **Frozen-exempt:** narrowing on deeply frozen values survives regardless. (We
  want this either way.)

### 7.2 Field narrowing vs. local narrowing

Current provisional rule bans field narrowing entirely. Is `local x = t.x` an
acceptable tax, or do we need limited field narrowing for real code to be
writable?

### 7.3 Conditional freeze

```lua
if cond then table.freeze(t) end
```

Must-analysis: frozen only if frozen on every path. Simplest rule is to only
recognize freeze in straight-line code at the allocation site. Is that too
strict?

### 7.4 `Frozen<T>` into a mutable parameter

Definitionally an error. Confirm the diagnostic is comprehensible, since this
will be the most common false-positive-feeling rejection.

### 7.5 Readonly indexer syntax

Does `{ read [string]: boolean }` parse in current Luau? Read/write modifiers on
**indexers** (vs. named properties) have been landing with the new solver.
**Verify against the actual parser** — if it parses, prefer it over the shim
alias; if not, shim.

Note read-only property syntax also fixes the width-subtyping variance hole:
`{read x: T}` is covariant where the mutable version must be invariant.

### 7.6 `Readonly` return types

Covariance means a mutable table flows into a read-only return with no variance
work — return position is the cheapest place to apply this. Confirm the rule that
**read-only is not assignable to mutable**, or the guarantee leaks the first time
someone passes a result to a helper taking a mutable table.

`next` and `pairs` are reads and stay fine. Writes error at the write site.

---

## 8. Test corpus design

**The corpus is the specification.** Rust's UI tests, TypeScript's baselines, and
Luau's own conformance suite all work this way.

### Format

One file per case, expectations inline, snapshot the full rendered diagnostic.

```lua
--!rime
local t = {}
local f = table.freeze(t)
t.x = 1
--~ ERROR[R0041]: cannot write to a frozen table
--~ NOTE: frozen at line 3
```

Inline beats a separate expectations file — case and expectation can't drift
apart. Snapshot the **rendered** diagnostic per §6.

### Three buckets, not two

```
tests/
  accept/               must typecheck clean
  reject-unsound/       must be rejected — can actually fail at runtime
  reject-conservative/  valid programs we reject anyway
```

**The third bucket is the important one.** It's the false-positive budget. It
determines whether people tolerate the tool. If it's mixed in with genuine errors
we can't tell a precision improvement from a soundness regression. Rust tracks
these as known limitations for exactly this reason.

Every case in it carries a comment saying **why** it's rejected and **what would
fix it** — which is also the text of the diagnostic, per §6 rule 7.

### Differential testing — the test that tests the claim

Make the accept corpus **executable**. Every accepted program runs under real
`luau` in CI. If it raises a type error at runtime, the checker was wrong.

That's ground truth rather than our beliefs, and given the theorem is "checks
clean => cannot raise a runtime type error," it's the only test that tests the
claim.

Pair with **mutation cases**: remove the freeze, confirm the program *does* throw
— proving the test would have caught it.

Roblox makes this awkward for anything touching `Instance`, so keep the
executable corpus to pure Luau; engine cases are static-only.

---

## 9. Starter cases

These pin the open decisions in §7.

**Freeze / const**

1. freeze fresh literal, write through the original binding -> reject (§5 wrinkle)
2. freeze inside `if`, write after -> must-analysis (§7.3)
3. `Frozen<T>` passed to a mutable parameter -> reject (§7.4)
4. `table.clone` of a frozen value, then write -> accept
5. `setmetatable` on a frozen table -> reject
6. deep vs shallow: frozen table containing an unfrozen one -> nested write
7. `table.isfrozen(t)` guard narrows -> accept
8. `local t: {a: number} = table.freeze({a=1})` -> reject, check the message
9. exported mutable table -> reject; exported frozen table -> accept

**Narrowing / yields**

10. narrowed local, `task.wait()`, use after -> reject (§7.1)
11. narrowed **frozen** local, `task.wait()`, use after -> accept
12. narrowed field vs. narrowed local -> §7.2
13. narrowed local, intervening function call, use after -> §7.1
14. narrowed local captured by a closure -> reject
15. `WaitForChild` result used after possible `Destroy` -> reject
16. yield inside a `Changed` handler -> reject

**Readonly**

17. `Readonly<T>` return, caller writes -> reject at write site
18. `Readonly<T>` return, caller `pairs` over it -> accept
19. `Readonly<T>` passed to mutable param -> reject
20. nested table under a `Readonly` handle, inner write -> decide transitivity (§7.6)

**Boundaries**

21. `OnServerEvent` payload used without a decoder -> reject
22. same, through a `--!trusted` decoder -> accept
23. `JSONDecode` result field access -> reject

**CFG (independent of §7 — write these first)**

24. `local x` assigned in one branch only, read after -> reject
25. same, assigned on every branch -> accept
26. function annotated `-> number` with a branch falling off the end -> reject
27. statement after `return` -> warn
28. `pcall` result discarded -> reject
29. key inserted into a table during `pairs` over it -> reject
30. constructor omits a field declared in the class type -> reject

---

## 10. Session plan

1. **Scaffold the harness first.** Runner, three bucket dirs, inline expectation
   parser, snapshot diff. Empty but working.
2. **Write cases 1–9** (freeze/const). Most settled semantics, gets the format
   shaken out.
3. **Draft the diagnostic renderer early** — even as a stub that prints spans and
   labels. Per §6 the messages are snapshotted, so the format wants to exist
   before twenty snapshots are baked against a placeholder.
4. **Write cases 24–30** (CFG). Settled semantics, no aliasing dependency, and
   they force the CFG to exist — which the yield tracking needs anyway.
5. **Argue §7.1 and §7.2** with cases 10–14 open in the editor. Don't write the
   checker until these are decided — they determine its shape.
6. **Wire up the differential runner** against real `luau` for the accept bucket.
7. **Fill in 15–23.**
8. Only then start on the checker itself, taking the corpus as the spec.

### Groundwork to check in the same session

- does `{ read [string]: boolean }` parse? (§7.5)
- crates.io / GitHub name availability
- diagnostic code scheme: settle `R0041` numbering ranges by category before
  writing cases that bake codes in
- pull the Roblox API dump + reflection metadata for `Instance` types — read how
  luau-lsp consumes it
- `default.project.json` parsing for path -> DataModel `require` resolution

### Existing Rust bases worth reading rather than starting cold

`full-moon` (lossless parser), `Selene`, `StyLua`, `luau-lsp`. For diagnostics
specifically: `ariadne` and `codespan-reporting` — worth picking one before
writing the renderer by hand.

---

## 11. Deferred

Not this session, but the shape to keep in mind:

- lossless CST with error recovery (rowan-style red/green) for LSP-quality
  incremental parsing
- query-based incremental engine (salsa / Roslyn), keyed on module signatures so
  a body edit invalidates nothing downstream
- arena allocation, string/type interning
- parallel module DAG once signatures are known
- server/client/module script context modelling (reject
  `game.Players.LocalPlayer` on the server)
- LSP code actions driven by the structured suggestions from §6 rule 6
- **Connection leaks.** A `RBXScriptConnection` never disconnected is Roblox's
  version of a resource leak and the most common memory bug in real games —
  nothing catches it today. C++ solves this with destructors and Java with
  try-with-resources; Luau has no scope-exit hook, so at best this is a
  heuristic lint rather than a sound check. Still worth having.
- **`error()` as a tracked effect.** Same machinery as yield tracking: mark
  functions that can raise, force a `pcall`. Java's checked exceptions are
  widely considered a design mistake, so approach with caution — but the
  infrastructure is free once yields are tracked.
- **User-written type predicates.** `function isFoo(x): x is Foo` — generalizes
  the `table.isfrozen` narrowing to user code, and is the only way to make
  boundary decoders (§4) narrow properly rather than just returning a type.
