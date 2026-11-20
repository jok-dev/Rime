# Rime — project rules

Working rules for anyone changing Rime, human or agent. `README.md` says
what Rime is and how to use it; `DESIGN.md` is the design. In code and
corpus comments, `plan §N` or a bare `§N` means section N of `DESIGN.md`.

## How the crates fit together

    source -> rime-syntax -> rime-hir -> rime-check -> rime-diag -> rime-cli

- `rime-syntax` is the only crate that names the parser (`full_moon`). It
  parses and lowers to HIR. Swapping the parser means rewriting this
  lowering and nothing else.
- `rime-hir` is the parser-independent tree every analysis reads. It has
  no dependencies. Keep it that way.
- `rime-check` holds the CFG, the dataflow framework, the project linker
  and the rules. It never depends on `full_moon`; its tests may use `mlua`.
  `Session` is the one pipeline behind both `rime check` and `rime lsp`.
- `rime-check` does not know Roblox project layout. `Resolver`
  (`crates/rime-check/src/project.rs`) is a trait: `rime-roblox` implements
  it for Rojo projects, and the corpus harness implements it for sibling
  files.
- `rime-diag` is the `Diagnostic` type, the rule names and defaults, and
  the renderers. It cannot depend on the checker.
- `rime-std` loads the standard-library definitions (the global names).
- `rime-custom` runs a project's own rule files (`rime-rules/*.luau`) in a
  sandboxed Luau VM. It is separate so that `mlua` stays out of
  `rime-check`'s dependencies.
- `rime-cli` is the binary: `rime.toml`, file discovery, output formats,
  the language server.
- `rime-bench` is benchmarks, the parser-fidelity check and the survey
  tools. It is not shipped.

## Adding a rule

1. Write the corpus cases first: at least one under `accept/` and one under
   a `reject-*` bucket of the rule's family in `tests/` (see "The corpus").
2. Give the rule a code and a name in `crates/rime-diag/src/lib.rs`: one
   `RULE_NAMES` entry and one `RULE_DEFAULTS` entry beside it (a unit test
   enforces one per code). The severity there must match what the rule
   constructs (`Diagnostic::error` or `Diagnostic::warning`). A rule that a
   project has to opt in to also goes in `OFF_BY_DEFAULT`.
3. Put the rule in its own module under `crates/rime-check/src/rules/`, or
   in the module of its family. The module's doc comment says why the rule
   exists and what it deliberately leaves out.
4. Call it from the drivers. A rule that needs one function body goes in
   `check_body` (`crates/rime-check/src/lib.rs`). A rule that needs the
   whole file goes in both `check_source_unguarded` (same file) and
   `check_file`, a free function in `crates/rime-check/src/session.rs`. A
   rule that needs other files goes in `check_file` only.
5. Add the rule to the tables in `README.md`.
6. Regenerate `examples/rime.toml` (see below) and accept the new snapshots
   with `cargo insta review`.

## The corpus

The corpus under `tests/` is the specification. Layout:
`tests/<family>/<bucket>/<case>.luau`, with three buckets:

- `accept/` must produce no diagnostic, and must also run without error
  under real Luau (`crates/rime-check/tests/differential.rs`). A case that
  cannot run outside Roblox, or that requires another file, says so with a
  line `--!no-exec` in its leading comment block.
- `reject-unsound/` must produce at least one error that is not marked
  conservative: the code can fail at run time.
- `reject-conservative/` is valid code Rime rejects anyway. Every
  diagnostic there must be marked `conservative`. Do not weaken this to
  make a test pass: it is what tells a precision improvement apart from a
  soundness regression.

Every case starts with `--!rime` and carries its expectations inline as
`--~ ERROR[R0041]: ...` lines. A directory that holds a `main.luau` is one
case: its files are checked together as a project, and the expectations
and the snapshot are `main.luau`'s. Rendered output is also snapshotted
with `insta` in the `snapshots/` folder beside the cases.

Some cases are red on purpose. Each specifies a rule that is not built
yet, so a plain `cargo test` exits non-zero until the rule lands. They are
listed in `tests/expected-red.txt`, one path per line.

`scripts/ci-test.sh` is the gate, on your machine and in CI. It runs the
whole suite and passes only when exactly the listed cases fail. Any other
failure is a regression. When a change makes a listed case pass, delete its
line from `tests/expected-red.txt` in the same change: the script fails
until the line is gone.

## Diagnostics: one per occurrence, always

Every occurrence of a defect gets its own diagnostic at its own site. NEVER
suppress, merge, or skip a diagnostic because another one for the same
name/defect already exists elsewhere — editors jump to a diagnostic's
primary file:line, so a hidden occurrence is an unnavigable one.

- A note connecting repeat occurrences is fine ("written 3 times in this
  file"); hiding any of them is not.
- Structural one-site defects are exempt: an unused binding has one
  declaration site, a duplicate key points at the duplicate (with the first
  as secondary label) — those ARE per-occurrence.
- Same-site cross-rule precedence (one event, the more specific rule wins)
  is fine; that's one site, one diagnostic.

## Traps

Facts about Luau, Roblox and this code base that are easy to get wrong:

- `table.freeze` returns the table it was given. Typing it as
  `<T>(T) -> Frozen<T>` and stopping there is unsound, because the original
  binding still looks mutable: the argument itself has to be retyped from
  that point on (`tests/freeze/reject-unsound/041_freeze_alias.luau`).
- `table.freeze` is shallow. Deep and shallow are different states
  (`FrozenState` in `crates/rime-check/src/rules/freeze.rs`); treating them
  as one makes the guarantee that narrowing survives a yield false one
  level down.
- `Instance` properties are never constant. The engine changes them, so no
  amount of freezing lets a narrowing on `part.Parent` survive a yield.
- A module's return value is cached by `require` and shared by every
  requirer, so an exported mutable table is shared global state.
- Luau has no `goto`: do not build control-flow machinery for it.
  `continue` jumps to the loop's latch, not its head, and in
  `repeat ... until c` the condition sees locals declared in the body.
- The parser is recursive, and deeply nested source needs a deep stack.
  `rime check` and `rime lsp` parse and check on threads with a large
  stack (`WORK_STACK_BYTES` in `crates/rime-cli/src/main.rs`). New code
  that parses or checks must run on those threads, not on the main thread.
- mlua's `load` parses and compiles. The parser-fidelity check is a sound
  test of what Luau accepts, but its timing is not Luau's parse time.
- Reports come in the order of the file walk, which sorts names in plain
  byte order (`crates/rime-check/src/walk.rs`). A build from before the
  walk was sorted lists files in the file system's own order, so to compare
  the output of two builds byte for byte, sort both outputs first.

## No blanket `cargo fmt`

The repo is not fmt-clean and the index has mixed line endings
(`core.autocrlf=false`, some files CRLF, some LF). A crate-wide `cargo fmt`
rewrites thousands of unrelated lines and flips EOLs. Match the surrounding
style by hand in the lines you touch.

## Regenerate `examples/rime.toml` before every release

`examples/rime.toml` is generated by `rime config-template` and lists every
rule with its current default severity. The checked-in file carries the
version string, and `crates/rime-cli/tests/config_template.rs` fails whenever
it drifts from the generator. So after adding a rule, changing a rule's
default, or bumping the workspace version, run

    cargo run -p rime-cli -- config-template --out examples/rime.toml

and commit the result.
