# Rime

Rime is a command-line static checker for Roblox Luau. Point it at a Rojo
project and it checks every module, and the requires and calls between
them: nil-safety with flow narrowing, frozen-table tracking, project-wide
call checking, control-flow checks, and unused code. It reads ordinary
`.luau` files and prints diagnostics — no build step, no new language,
nothing to migrate away from — and what it does today fits in one sentence:

> Rime reports the defects it can see in the source: a call that does not
> fit its function, a `require` that reaches no module, a write to a frozen
> table, a value annotated as optional used where `nil` throws.

Rime 0.1.0 does not infer types: where a check needs a type, it reads the
annotation you wrote. So a clean run means the rules found nothing. It does
not prove that a module cannot fail at run time. A report Rime cannot prove
is marked conservative: `(conservative)` in the text output, with or
without `--plain`, and `"conservative": true` with `--format json`.

## Why use Rime

- **Fewer runtime surprises.** The bug classes that actually page you —
  nil derefs on a value annotated `Instance?`, calls left behind when a
  function changed, requires of deleted modules — become check-time errors.
- **Refactor with confidence.** Calls into a module, require paths, and the
  members a module exports are checked project-wide.
- **Excellent error messages.** Every diagnostic carries labeled spans,
  nearly all the reasoning as a note, and many a `Help:` saying how to fix
  it.
- **Super fast.** Whole-project checks in seconds; tight enough to run on
  every save.
- **Highly configurable.** A rule is set to `"error"`, `"warn"` or
  `"off"` in `rime.toml`; one-site suppressions are inline, reasoned, and
  greppable.
  Adopt incrementally on legacy code: start a rule at `"warn"` and raise it
  to `"error"` when the code is clean.
- **Built for the AI loop.** Agents write plausible Luau; Rime checks it
  against the project it has to fit into. Self-contained diagnostics make
  fix-and-rerun work without a human interpreting errors.
- **First-class Rojo support.** Built for how serious studios ship: filesystem
  source, git, Rojo, CI. Rime reads your Rojo project file directly —
  mounts, DataModel paths, requires.

## The dialect

**Strict dialect** means a strict *subset* of Luau, not a new language: every
file Rime reads is ordinary `.luau`. No build step, no source maps; the
existing toolchain (luau-lsp, StyLua, Selene, Rojo) keeps working untouched.
The only thing Rime adds to a file is a comment,
`-- rime: allow(<rule>, "reason")`. In 0.1.0 the subset bans less than the
design in `DESIGN.md` describes:

- **Annotations are read, not required.** A check that needs a type takes
  it from the annotation — a local declared `Instance?`, a parameter's
  type, a return type — and says nothing about a value that has none. An
  unannotated function is not an error, and neither is `any`, a `::` cast,
  `getfenv`, `setfenv` or `loadstring`.
- **Banned:** implicit globals, reads of undefined names, a local that
  shadows another local or a parameter of the same function, and, in a
  Rojo project, a dynamic `require` and a `require` of a module that is
  not there.
- **Nil-safety** with flow narrowing, for locals and parameters annotated
  as optional: after
  `local door: Instance? = workspace:FindFirstChild("Door")` you must prove
  `door` before use. The same result in an unannotated local, an optional
  field, and the result of a call are not checked.

## Top features

- **CFG-derived checks** — definite assignment, definite return, unreachable
  code, and an assignment into a table inside the `pairs` or `ipairs` loop
  over it, other than through the loop's own key, when the table is a
  parameter or a local of the same function.
- **Cross-file call conformance** — extra arguments, `.`/`:` confusion, and,
  where the parameter is annotated, a missing argument, or a string, number,
  boolean or `nil` literal that does not fit a parameter annotated `number`,
  `string` or `boolean`, checked project-wide against each module's exported
  interface.
- **Freeze/const tracking** — `table.freeze` is followed through control
  flow, inside one function or a module's top level. A write to a table
  that is frozen on every path is an error when it goes through the local
  that was frozen: `t` after `table.freeze(t)` on a line of its own, `t`
  and `f` after `local f = table.freeze(t)`. A second name for the table
  (`local g = f`), a parameter, and a `table.freeze` call written anywhere
  else (`f = table.freeze(t)`, `print(table.freeze(t))`) are not followed.
  Neither is a local declared outside the function, nor a write made by a
  call (`table.insert(t, v)`, `rawset`). Reassigning a `const` is an error
  too.
- **First-class Rojo support** — reads the project file's mounts, walks DataModel
  paths (`game:GetService`, `WaitForChild`, `script.Parent`) and string
  requires (`"./x"`, `"@self/x"`, `.luaurc` aliases), and verifies
  every static `require` lands on a real module file, and reports load-time
  require cycles before Roblox does.
- **Unused-code detection** — unused locals, parameters, and functions, plus
  dataflow-verified dead stores, and module exports nothing in the project
  uses. An unused export comes with the exact range to delete (`extent`
  with `--format json`).

## Install

Every [release](https://github.com/jok-dev/Rime/releases) has one
executable per system. Each is a single file with nothing to unpack:

| File               | Runs on |
| ------------------ | ------- |
| `rime-linux`       | Linux, x86-64, glibc (built on Ubuntu 22.04) |
| `rime-macos`       | macOS, Apple silicon (arm64) only |
| `rime-windows.exe` | Windows, x86-64 |

Download the file for your system and `SHA256SUMS` from the same release,
check the one against the other, and put the executable on your `PATH`
as `rime`.

Linux:

```bash
curl -LO https://github.com/jok-dev/Rime/releases/latest/download/rime-linux
curl -LO https://github.com/jok-dev/Rime/releases/latest/download/SHA256SUMS
grep rime-linux SHA256SUMS | sha256sum --check
chmod +x rime-linux    # a download does not keep the executable bit
./rime-linux --version
sudo mv rime-linux /usr/local/bin/rime    # on your PATH, as `rime`
```

macOS:

```bash
curl -LO https://github.com/jok-dev/Rime/releases/latest/download/rime-macos
curl -LO https://github.com/jok-dev/Rime/releases/latest/download/SHA256SUMS
grep rime-macos SHA256SUMS | shasum -a 256 --check
chmod +x rime-macos
./rime-macos --version
sudo mkdir -p /usr/local/bin && sudo mv rime-macos /usr/local/bin/rime
```

A file downloaded with a browser instead of `curl` is quarantined, and
macOS refuses to run it until the flag is removed:

```bash
xattr -d com.apple.quarantine rime-macos
```

Windows (PowerShell):

```powershell
Invoke-WebRequest https://github.com/jok-dev/Rime/releases/latest/download/rime-windows.exe -OutFile rime.exe
Invoke-WebRequest https://github.com/jok-dev/Rime/releases/latest/download/SHA256SUMS -OutFile SHA256SUMS
(Get-FileHash rime.exe -Algorithm SHA256).Hash -eq (Select-String rime-windows.exe SHA256SUMS).Line.Split(' ')[0]   # True
.\rime.exe --version
```

To pin a version, replace `latest/download` with `download/v0.1.0`. The
release also holds `LICENSE`, `THIRD-PARTY-NOTICES.txt` and
`COPYRIGHT-library.html`: the license texts for everything built into the
executables.

Then, at the root of a Rojo project:

```bash
rime check .
```

To build from source instead, see [Development](#development). The VS Code
extension is in [`editors/vscode`](editors/vscode) and is built from source.

## Configuration

By default most rules are errors. The rules about unused code and style,
and some others whose reports are often deliberate code, are warnings, and
six convention rules are off. [`examples/rime.toml`](examples/rime.toml) lists
every rule with its default. Everything is configurable from there:
downgrade any rule to `"warn"` or `"off"` in `rime.toml` (by name or raw
code), raise one that warns by default to `"error"` so it fails the run,
scope exceptions with `ignore` directories, vouch for runtime-only paths
with `[requires] trust`, or suppress one site inline with
`-- rime: allow(<rule>, "reason")` — explicit, greppable, auditable in
review. Only `error`-severity diagnostics fail the exit code.

Four of the warnings are rules whose reports are often deliberate code,
which is why they do not fail a run until a project raises them:

| Code  | Name                   | Flags |
| ----- | ---------------------- | ----- |
| R0308 | `module-write`         | a write into another module's export table |
| R0602 | `divide-by-zero`       | a division or modulo by a literal zero |
| R0801 | `color3-bounds`        | `Color3.new` with a channel outside 0–1, as in `Color3.new(255, 0, 0)` |
| R0804 | `suspicious-udim2-new` | `UDim2.new` with one to three arguments; it takes four |

Six rules are opt-in: each is off until `rime.toml` gives it a level
(`"warn"` or `"error"`), because it checks a convention a project has to
choose, not a defect. `--only <rule>` also shows one without editing the
file. Any other setting for one of them, `conservative` included, needs a
`level` beside it: without one the rule would stay off, so `rime.toml` is
refused with a message that says so.

- `module-scope-computation` (R0312): what a module may do as it loads.
  It takes settings of its own (see [Opt-in rules](#opt-in-rules)).
- `pcall-discarded` (R0110), `exported-function-reassign` (R0305),
  `module-name-mismatch` (R0310), `enumeration-require` (R0508) and
  `mixed-table` (R0703).

An inline allow on a line of its own covers the next line; at the end of a
line of code it covers that line. A report belongs to the line its label
starts on. R0304 `call-extra-args` also belongs to the line its call
starts on, so the comment can sit above a call written over several lines:

```lua
-- rime: allow(call-extra-args, "old callers still pass the amount")
Shop.buy(
    player,
    amount
)
```

### Exit status

`rime check` exits with:

| Status | Meaning |
| ------ | ------- |
| `0`    | No `error`-severity diagnostics. |
| `1`    | At least one `error`-severity diagnostic — or `rime.toml` / `--only` could not be read. |
| `2`    | The command could not be run as written: a malformed command line, or a path to check that does not exist. Nothing was checked. |
| `101`  | Rime crashed on part of the run (`R0901 internal-error`). The results are incomplete, whatever else was found. |

A crash inside Rime while it works on one file is contained to that file:
it is reported there as `R0901`, with where Rime panicked and which rule
was running, the other files are still checked, and the run exits `101`
rather than `0` or `1` — so a CI step can tell "Rime broke" from "the code
has findings", and neither reads as a pass. `R0901` cannot be turned off,
downgraded, filtered by `--only`, `ignore`d or allowed inline. With
`--format json` the count is `summary.internalErrors`. A crash that cannot
be contained (a stack overflow, an abort) still ends the process with the
platform's own non-zero status.

A path that exists but holds no `.lua`/`.luau` file is not an error — the
run exits `0` — but it is warned about on stderr, per path, so "checked 0
files" is not mistaken for a pass that checked something.

### Project file and requires

Cross-file checks need a Rojo project file. Rime uses, in order:
`--project <file>`, `project = "<file>"` in `rime.toml`, then the nearest
folder at or above the first checked path that holds a `*.project.json`
(`.project.jsonc` too). In that folder `default.project.json` wins and a
single file under another name is used. Several files and no default are
not guessed between. Every run says on stderr which file it uses, or that
cross-file checking is off and why.

A `require` is followed when it is an instance path
(`require(script.Parent.X)`) or a Luau string path:

| Written | Means |
| ------- | ----- |
| `"./x"`, `"../x"` | relative to the requiring module's parent (`script.Parent.x`) |
| `"@self/x"` | a child of the requiring module (`script.x`) |
| `"@game/A/B"` | from the DataModel root |
| `"@name/x"` | `name` from the `aliases` of the nearest `.luaurc` |

`foo/init.luau` is the module `foo`, as in Luau. A relative path in an
`init` file that only resolves from the file's own folder (the older rule,
which darklua still uses) is followed too, and the run says so.

Every part of a path is case-sensitive, on every operating system, for an
instance path and a string path alike: `script.Parent.Util.helper` and
`"./Util/helper"` do not find `util/helper.luau`. Roblox instance names
are case-sensitive and so is a Linux disk, so Rime reports a broken
require (`R0501`) even where Windows or macOS would open the file.

A require Rime cannot follow is not reported per call. A string with no
`./`, `../` or `@` (a loader's `require("Name")`), an alias no `.luaurc`
defines, and calls through a rebound `require` (`local require = ...`) are
each counted and said once per run on stderr (`rime: note: ...`), and in
`notes` with `--format json`.

### Which files are checked

`rime check <paths>` checks every `.lua`/`.luau` file under each path
(default `.`), except what git ignores: inside a git work tree, anything
matched by a `.gitignore`, `.git/info/exclude` or your global excludes file
is skipped, the way `git status` and ripgrep skip it. So `rime check .` at
the top of a repository is about the repository, not its build output,
package caches or other checkouts kept in ignored folders. Outside a git
work tree a `.gitignore` means nothing and nothing is skipped.

- `--no-gitignore` turns this off and checks everything under the paths.
- A path you name is always walked, ignored or not (`rime check Packages`);
  the rules apply to what is found under it.
- Matching is by pattern, without asking git: a file that is tracked but
  also matches a pattern is skipped, where git would still list it.
- Files under a Rojo mount are still read when git ignores them, because
  the code you check requires into them (`Packages/` from wally is the
  usual case). They are resolved against, never reported on — the same as
  when you run `rime check src` and the mount sits outside the path.
- `ignore` in `rime.toml` is separate and applies on top. A file listed
  there produces no reports, but in a Rojo project it is still read and
  checked, and its findings are counted as "ignored" in the summary. A
  git-ignored file outside a mount is not read at all, so it is counted
  nowhere. `--no-gitignore` does not switch `ignore` off, and naming a
  path does not get past it.
- The language server (`rime lsp`) does not apply this yet: it still lists
  every file under the workspace folder.

Reports come file by file, in an order that is the same on every operating
system: first the files under the Rojo project's mounts, if there is a
project, then the remaining files, path by path as you named the paths, and
within the mounts and within each path sorted by name, folder by folder, in
plain byte order (`Zeta.luau` before `alpha.luau`, `10.luau` before `9.luau`).

A filled-out `rime.toml`, placed at the project root next to the Rojo
project file:

```toml
# Everything but six convention rules (listed above) is ON by default,
# most rules at error severity. The config
# sets a rule's level as a plain string ("error"/"warn"/"off") or, for
# finer control, a { level, conservative } table. "warn" keeps a rule
# visible without failing the run, "off" suppresses it, and "error" makes
# a rule that warns by default fail the run.

# Directories that produce no reports. Interfaces are still extracted
# from ignored files, so cross-file checking of the rest is unaffected.
ignore = ["Packages/_Index", "third-party"]

[requires]
# DataModel paths whose requires you vouch for when static resolution
# misses — e.g. subtrees populated by a runtime loader. `*` is one name,
# `**` a whole subtree; without them an entry is one module.
trust = ["game.ReplicatedStorage.Shared.Runtime.**"]

[std]
# Which global names `undefined-variable` (R0022) accepts. Rime bundles
# selene's Luau + Roblox std files, so out of the box `game`, `workspace`,
# `task`, `Instance` and the rest are known and you need none of this.
#
# `files` REPLACES the bundled set with your own selene-format std YAML
# (paths relative to this file; a missing one is an error, not a silent
# no-op) — reach for it when you target a non-Roblox Luau host. Left off
# here on purpose: setting it on a Roblox project drops `game` with it.
# files = ["std/my-host.yml"]
#
# `globals` ADDS bare names on top of whichever set `files` selected —
# the usual dial. Frameworks that inject globals into your scripts go
# here: test runners, build-time constants, plugin-only Roblox globals.
globals = ["describe", "it", "expect", "PluginManager"]

[names]
# Variable names the player-map rules (R0802, R0803) read as "this holds
# a Player" when one is used as a table key. A `Player` annotation always
# counts. `*` matches any run of characters. Default: ["player", "plr"].
player = ["player", "plr", "client", "*Player"]

[custom-rules]
# The directory a custom rule (rime-rules/*.luau) sees file paths, and
# matches its `files` globs, relative to. Default: this file's directory.
# Set it when this file is not at the top of what you check, e.g. a
# rime.toml under tools/ that checks src/: rules never run on files
# outside it, and the run prints a note saying how many it left out.
# root = ".."

[conservative]
# Reports Rime marks `(conservative)` are its stated limitations, not
# proof of a bug. This downgrades every such report on top of its
# rule's level; it never raises one. Default: the rule's own level.
default = "warn"

[rules]
# Typical migration dials for a legacy codebase: keep the signal
# visible, gate CI only on what's already clean.
possible-nil-use = { level = "error", conservative = "off" }  # keep firm nil claims, drop what Rime cannot classify
shadowing        = "warn"   # repeated `local ok, err = pcall(...)` chains
unused-param     = "off"    # event handlers take args by signature
dead-store       = "warn"
dynamic-require  = "warn"
R0103            = "off"    # raw codes work too (this is unreachable-code)
mixed-table      = "warn"   # off by default; a level turns it on
```

## Layout

```
crates/
  rime-syntax   the only crate that names the parser; parse + lower to HIR
  rime-hir      parser-independent HIR: flat arenas, precomputed nil-admission
  rime-check    CFG + dataflow + the rules (one module per rule family)
  rime-diag     Diagnostic, rule names and defaults, rendering   <- no dep on the checker
  rime-std      standard-library definitions (which globals exist)
  rime-roblox   Rojo project resolution
  rime-custom   a project's own rules (`rime-rules/*.luau`) in a sandboxed Luau VM
  rime-cli      the binary
  rime-bench    benchmarks, fidelity oracle, code surveys (not shipped)

tests/
  <feature>/             cfg | freeze | calls | narrowing | unused ...
    accept/              must check clean AND run clean under real Luau
    reject-unsound/      must be rejected — can actually fail at runtime
    reject-conservative/ valid code we reject anyway  <- the false-positive budget
```

`rime-diag` deliberately cannot depend on the checker. A `Diagnostic` is
self-contained data, which is what lets the same value render to a terminal,
snapshot in a test, or become an LSP code action without a rewrite (§6).

Design notes and the full plan live in `DESIGN.md`; section references here
point there. The working rules for contributors are in `CLAUDE.md`: how the
crates fit together, how to add a rule, and how the corpus works, including
the cases that are red on purpose.

## Development

```bash
cargo build
cargo test -p rime-check                       # corpus + differential harnesses
scripts/ci-test.sh                             # the whole suite, the way CI gates on it
cargo insta review                             # accept snapshot changes
cargo run --release -p rime-cli -- check <dir> # check a project
```

The corpus is the specification (§8): every rule lands as test cases first,
so some `reject-*` cases stay red until their rule is built — the harness
names them. `accept/` cases additionally execute under real Luau (mlua), so
"checks clean" is tested against "runs clean".

Those red cases are listed in `tests/expected-red.txt`, and they are why a
plain `cargo test` exits non-zero. `scripts/ci-test.sh` passes when exactly
the listed cases fail: any other failure fails it, and so does a listed case
that passes, until its line is deleted.

Parser fidelity has its own check, run by hand: `./scripts/fetch-corpus.sh`
then `cargo run --release --bin rime-fidelity` exits non-zero on any
valid-Luau-rejected gap. CI does not run it, because it clones other
projects at their latest commit. Thresholds and methodology are in
`DESIGN.md`.

## Corpus conventions

Every case starts with `--!rime`. Expectations are inline so a case and its
expectation cannot drift apart:

```lua
--!rime
local t = {}
local f = table.freeze(t)
t.x = 1
--~ ERROR[R0041]: cannot write to a frozen table
--~ NOTE: frozen at line 3
```

The harness enforces the bucket contract, and the third bucket is the one
that keeps everyone honest: **every diagnostic in `reject-conservative/`
must be flagged `conservative: true`**. If a rule ever rejects valid code
without saying so, the case lands in the wrong bucket and CI fails. That is
what makes a precision improvement distinguishable from a soundness
regression.

Cases in that bucket carry a comment saying why they are rejected and what
would fix it — which is also the text of the diagnostic (§6 rule 7).

### Diagnostic code ranges

```
R00xx  syntax and scope (globals, shadowing, unused, `table.freeze`, `const`)
R01xx  control flow (definite assignment, definite return, unreachable, dead stores)
R02xx  nil narrowing
R03xx  calls and module members (arity, argument types, `.`/`:` confusion, unknown and unused members)
R05xx  require & project graph (broken, dynamic, outside-project, circular)
R06xx  suspicious patterns (almost-swapped, zero divisors, duplicate branches)
R07xx  style (empty blocks, mixed tables, manual clones)
R08xx  Roblox checks (Color3 and UDim2 constructor misuse, Player-keyed tables never cleared)
R09xx  internal (ICE, unsupported syntax)
```

The globals family in R00xx is the one that reads `[std]` above — what
counts as a known global is exactly what those three rules are checking
against:

| Code  | Name                 | Severity | Flags |
| ----- | -------------------- | -------- | ----- |
| R0022 | `undefined-variable` | error    | a read of a name that is not a local, not a `[std]` global, and never written as a global in the same file; or `...` read in a function whose parameter list has no `...` |
| R0023 | `unscoped-variable`  | error    | a write to a name with no `local` in any enclosing scope — an implicit global. `_`-prefixed names are exempt |
| R0024 | `global-usage`       | warn     | any use of `_G` or `shared`, Luau's ambient mutable-global handles, unless shadowed by a local |

R0024 warns rather than errors because reaching for `_G` is a deliberate
act; R0023's identical-looking write usually is not, which is the whole
difference between the two.

`const x = e` and `const function f() end` are checked as the locals they
are — scope, shadowing, unused, narrowing — and have one rule of their own:

| Code  | Name             | Severity | Flags |
| ----- | ---------------- | -------- | ----- |
| R0043 | `const-reassign` | error    | an assignment to a name declared `const` — `x = e`, `x += e`, one target of `a, x = …`, or `function x() end` — in the declaring function or any closure inside it. Luau refuses to compile such a file. A write through the name (`x.field = e`) is not one |

`if local x = e then` and `if const x = e then` are experimental Luau and
are not checked yet: a file that uses either gets one R0900 that says so,
and nothing else in that file is checked.

Four rules read what the parser saw and the checker's tree does not keep
— attributes, parentheses, how a string was escaped:

| Code  | Name                         | Severity | Flags |
| ----- | ---------------------------- | -------- | ----- |
| R0044 | `unknown-function-attribute` | error    | an attribute other than `@native`, `@checked` and `@deprecated`, or one given twice, on any function. Luau refuses to compile such a file |
| R0611 | `bad-string-escape`          | error    | an escape Luau refuses to compile (`\300`, `\xZ`, `\u1234`, `\u{110000}`); or, conservative, a backslash before a character that is no escape (`\d`, `C:\Users`), which Luau reads as the bare character |
| R0706 | `parenthesized-condition`    | warn     | an `if`, `elseif`, `while` or `until` condition wholly inside parentheses. Carries the edit |
| R0707 | `needless-quote-escape`      | warn     | `\'` in a double-quoted string or `\"` in a single-quoted one. Carries the edit |

The attribute list form (`@[native]`) is not parsed yet: a file that uses
it gets an R0900.

Two rules look at calls by the name written at the call:

| Code  | Name                    | Severity | Flags |
| ----- | ----------------------- | -------- | ----- |
| R0304 | `call-extra-args`       | error    | more arguments than the function has parameters. For a plain name (`f(1, 2)`) every function the variable can hold is looked up through scope, wherever it is declared; for `T.f(...)` and `T:m(...)` the declaration in this file or the required module. Silent when any definition takes `...`, or the variable can hold something that is not a function written in the file |
| R0708 | `must-use`              | warn     | a call to a library function that only returns a value, on a line of its own: `math.max(a, b)`, `tostring(x)`, `table.concat(t)`. The list is fixed (216 names, plus `Enum.<Name>.GetEnumItems`, `Enum.GetEnums` and `game.GetService` called with a dot) |

### Opt-in rules

R0312 is the opt-in rule that takes settings of its own:

| Code  | Name                       | Severity      | Flags |
| ----- | -------------------------- | ------------- | ----- |
| R0312 | `module-scope-computation` | off (`warn`)  | work a module does as it loads: a call that is not allowed, or a loop. Every report is conservative: the code is valid Luau, and the rule is the project's convention |

A module's top level runs once, on the first `require`, before the
requirer gets anything back. A signal connected there, a thread started
there or an instance waited for there happens as a side effect of someone
naming the module. The rule keeps the top level to declarations.

```toml
[rules.module-scope-computation]
level = "error"                                # "warn" or "error" turns it on
allow-builtin = ["luau", "roblox-datatypes"]   # the default: both lists
allow = ["Signal.new", "*:IsServer"]           # the project's own, added to them
deny = ["Random.new"]                          # taken back out
allow-loops = false
exclude = ["tools/**", "*.spec.luau"]          # files the rule leaves alone
```

- **A call** is one report, wherever it sits in a top-level statement: a
  `local` initializer, a table field, a condition, the `return`, an
  argument of another call. Each call of a chain is its own report.
- **What is allowed without a setting.** `require` and `game:GetService`
  always, and two built-in lists of calls that give the same answer on
  every load and change nothing outside the module:
  - `luau`: the functions of `math`, `string`, `table`, `bit32`, `utf8`,
    `buffer` and `vector`, and `getmetatable`, `setmetatable`, `ipairs`,
    `pairs`, `next`, `rawequal`, `rawget`, `rawlen`, `rawset`, `select`,
    `tonumber`, `tostring`, `type`, `typeof`, `unpack`. Not `math.random`
    or `math.randomseed`, not `print`, `error`, `assert`, `pcall`, and
    not the `os`, `coroutine`, `debug` and `task` libraries.
  - `roblox-datatypes`: the constructors of Roblox's value types
    (`Vector3.new`, `CFrame.Angles`, `Color3.fromRGB`, `TweenInfo.new`,
    `UDim2.fromScale` and the like). Not `Instance.new`, `SharedTable.*`,
    `DateTime.now` or `BrickColor.random`, and no method of a service or
    an instance.

  `allow-builtin` says which lists are in use; `[]` uses neither. The
  lists name globals: after `local math = require(MyMath)`, `math.floor`
  is the project's own function and is reported. `local math = math` is
  still the library. The same goes for `require`: a project that puts
  its own function in a local named `require` adds `allow = ["require"]`.
- **A method called on a value** is not on a list, whatever the value:
  `("x"):rep(3)`, `name:upper()` and `CFrame.new():Inverse()` are
  reported. `string.rep("x", 3)` is the allowed way to write the first.
- **`allow`** adds callees, as they are written in the source:
  `Instance.new`, `Players.PlayerAdded:Connect`. `*` stands for any run of
  characters, so `Signal.*`, `*.new` and `*:IsServer` are patterns. A call
  on a call's result is written with `()`:
  `Storage:WaitForChild():WaitForChild`. The callee is matched as text,
  not resolved: after `local new = Instance.new`, `new("Part")` is the
  callee `new`. An entry has no spaces and no `()` at its end; one that
  has is refused when `rime.toml` is read.
- **`deny`** takes callees back out, with the same patterns. It wins over
  `allow`, the built-in lists, and `require` / `game:GetService`.
- **A loop** (`for`, `while`, `repeat`) is one report, and nothing inside
  it is reported one by one. With `allow-loops = true` loops pass and the
  calls inside them are checked.
- **Not reported:** `local`s, assignments, table and other literals,
  operators, indexing, `if` and `do` (what they hold is checked), function
  definitions, types, the `return`. A function body is not module scope.
  The exception is a function that runs on the spot: one written as an
  argument of `pcall`, `xpcall`, `table.sort`, `table.foreach`,
  `table.foreachi` or `string.gsub`, when that call is allowed. Any other
  call only keeps the function it is handed
  (`table.insert(handlers, function() ... end)`), and its body is not
  checked.
- **`scripts`** names the files that are entry scripts and are left alone.
  The default is the names Rojo makes a script of: `*.server`, `*.client`
  and `*.plugin`, each as `.lua` and `.luau`. Setting it replaces them;
  a project whose Rojo `syncRules` make scripts of other names lists its
  patterns here. A file that returns no value is never treated as a
  module, whatever its name.
- **`exclude`** names other files and folders the rule leaves alone
  (generated code, a vendored fork, tests). It is empty by default and is
  read beside `scripts`. This is the rule's own setting: the top-level
  `ignore` drops a folder's reports for every rule, and no other setting
  switches one rule off per path.
- **Path patterns** (`scripts`, `exclude`): without `/` a file name; with
  `/` a path below `rime.toml`, where `*` stays inside one folder and `**`
  does not. A pattern whose last part has no `*` is also a folder below
  `rime.toml`, with everything under it: `tools`, `tools/` and `tools/**`
  leave out the same files. Either slash works, and a `./` or `/` at the
  start is dropped: `./src/tools` and `src\tools` are `src/tools`. An
  entry whose folder is not there gets a warning when Rime starts.
- One site is let through with `-- rime: allow(module-scope-computation,
  "reason")` on its line or the line above. Above a statement it covers
  every call in the statement, however many lines that is written over,
  and on a loop it covers the loop. `allow-file` at the top of a file
  covers the file.
- Every report of this rule is conservative, so `[conservative] default`
  applies to it: under `default = "warn"` the rule cannot fail the run,
  and under `default = "off"` it reports nothing. Add
  `conservative = "error"` (or `"warn"`) to the rule's table to give it
  its own level.

Two rules work together when you delete dead code:

| Code  | Name             | Severity | Flags |
| ----- | ---------------- | -------- | ----- |
| R0309 | `unused-export`  | warn     | a member of a module's exported table that no file uses: no file that requires the module reads it, and the module does not read it off its own export. The module's own `M.NAME = value` defines the member and is not a use. Conservative. Project mode only |
| R0306 | `unknown-member` | error    | a call to a member the module does not have: `Mod.f()` on a required module, or `M.f()` inside the file that builds `M`. So a call left behind after its function is deleted is reported, in the same file as well as in others. Conservative |

When the project has a `require` Rime cannot follow (a loader that
requires modules by a computed path), R0309 is more careful, and the
report says so:

- A member is counted as used when anything in the project reads a member
  of that name off any value.
- A member whose name is also written as a string literal somewhere is
  still reported, with a mark. Such a loader may call members by names it
  takes from a list of strings, so the string may be a use; more often it
  is something else of the same name. The report has a note that names
  the places (`the name also appears as a string, at Hooks.luau:12, ...`),
  for you to check before deleting. Strings inside the member's own
  definition do not count: inside its function, or in the value it is
  given (`M.Opened = "Opened"`).

Never reported by R0309: a name that starts with `_`, and any member of a
module that is indexed with a computed key (`Mod[key]`) or passed around
as a value. Inside a module's own methods `self` is the module: a member
used as `self.name` is used, and a `self` that is passed on counts as the
module being passed around.

With `--format json`, every diagnostic has two keys for tools that work
through the R0309 list, `extent` and `stringMatches`:

```json
"extent": {
  "span":  { "start": 123, "end": 456 },
  "start": { "line": 6, "column": 1 },
  "end":   { "line": 9, "column": 4 }
},
"stringMatches": [
  { "path": "src/Hooks.luau", "line": 12 }
]
```

`extent` is the whole definition of the unused member, so a tool can
count the dead lines or delete them. `span` is byte offsets into the
file; `start` and `end` are 1-based line and column. The end is
exclusive. Deleting exactly this range removes the definition and leaves
a file that parses: the whole statement for `function M.name() ... end`
and `M.name = value`, with the `;` that ends it if there is one, or the
field and the `,` after it for a field of the table the module returns.
Comments above the definition are not part of it. `extent` is `null` for
every other rule, and for a member that is defined in more than one
place.

A file that parses is not always a file that behaves the same. A value
can run code when the module loads (`M.connection = connect()`), and
deleting the member deletes that call too; Rime does not tell such a
value from a plain one. One extent can also sit inside another, when a
member is defined inside a function that is itself unused: delete the
outer one.

`stringMatches` lists every line where the member's name is written as a
string, as described above: `path` as a diagnostic's `path` is written,
`line` 1-based. It is an empty list when there is none, and for every
other rule. A tool that deletes without a person reading the report
should skip a report where the list is not empty.

Deleting one unused export can leave another: a member that only the
deleted code used is reported on the next run. Run Rime again after
deleting until the list is empty.

R0306 goes by what the file that builds `M` writes: `M.name = ...`,
`function M.name()`, and `self.name = ...` in the module's own methods.
Only a method written directly as a member counts (`function M:init()`,
`M.init = function(self)`), not a function defined elsewhere and then
assigned (`M.init = init`). That holds for a call inside that file and
for a call from a file that requires it. A member that other code adds
at run time, such as a loader that sets a field on every module it
requires, is not seen, and a call to it is reported. Declare it in the
file (`M.name = nil` is enough) or allow the report there. R0306 says
nothing about a module whose export, or `self` in one of its methods, is
passed to other code or written with a computed key.

## License

[MPL-2.0](LICENSE) — file-level copyleft: changes to Rime's own files stay
open, while using Rime alongside proprietary code is unrestricted. Same
license as `full_moon`, the parser Rime builds on. A project's own
`rime.toml` and custom rule files (`rime-rules/`) are that project's own
work.

Parts that come from other projects:

- [Selene](https://github.com/Kampfkarren/selene), MPL-2.0: the
  standard-library files in `crates/rime-std/std/` (see the `NOTICE.md`
  there), the list of functions whose result must be used in
  `crates/rime-check/src/rules/must_use.rs`, and the test snippets in
  `crates/rime-check/src/rules/` whose comments name a Selene fixture.
- The Luau grammar in `editors/vscode/syntaxes/`, MIT (see
  `luau.tmLanguage.LICENSE` there).
- The file icon in `editors/vscode/icons/`, the "moon" icon of Feather,
  MIT (see `luau.svg.LICENSE` there).
