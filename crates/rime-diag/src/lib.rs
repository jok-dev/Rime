//! Diagnostics for Rime.
//!
//! Deliberately has no dependency on the checker: a `Diagnostic` is
//! self-contained data. That is what lets the same value be rendered to a
//! terminal, snapshotted in tests, or turned into an LSP code action without
//! any of those paths knowing about type representation.
//!
//! See plan §6. The nine rules there are the spec for this module; the ones
//! with teeth are:
//!   - every diagnostic answers what/where/why/how-to-fix
//!   - most Rime errors relate *two* places, so `labels` is plural by design
//!   - never render an internal type (there is no `TypeId` in this crate)

use std::ops::Range;
use std::path::PathBuf;

pub type Span = Range<usize>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// Stable diagnostic code, e.g. `R0041`.
///
/// Ranges are reserved by category so cases can be written before the checker
/// exists without renumbering later (plan §10, groundwork).
///
///   R00xx  syntax / dialect rules (annotations, `::`, `any`, globals)
///   R01xx  control flow (definite assignment, definite return, unreachable)
///   R02xx  narrowing, aliasing, yields
///   R03xx  call conformance (arity, argument types, `.`/`:` confusion)
///   R04xx  freeze / readonly / const
///   R06xx  untrusted boundaries
///   R09xx  internal (ICE, unsupported syntax)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Code(pub u16);

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "R{:04}", self.0)
    }
}

/// Stable names for diagnostic codes — the keys rime.toml accepts, and what
/// the terminal renderer shows next to the raw code so users can find them.
/// Raw `R####` keys always work too; names are added as rules land.
pub const RULE_NAMES: &[(&str, u16)] = &[
    ("shadowing", 11),
    ("duplicate-table-key", 12),
    ("unused-local", 13),
    ("unused-param", 14),
    ("unused-function", 15),
    ("unused-assignment", 16),
    ("faint-local", 17),
    ("malformed-allow", 21),
    ("undefined-variable", 22),
    ("unscoped-variable", 23),
    ("global-usage", 24),
    ("custom-rule-error", 25),
    ("freeze-write", 41),
    ("freeze-setmetatable", 42),
    ("const-reassign", 43),
    ("unknown-function-attribute", 44),
    ("definite-assignment", 101),
    ("definite-return", 102),
    ("unreachable-code", 103),
    ("dead-store", 104),
    ("pcall-discarded", 110),
    ("pairs-mutation", 111),
    ("possible-nil-use", 201),
    ("use-before-guard", 202),
    ("call-missing-arg", 301),
    ("call-type-mismatch", 302),
    ("call-method-confusion", 303),
    ("call-extra-args", 304),
    ("exported-function-reassign", 305),
    ("unknown-member", 306),
    ("unknown-field", 307),
    ("module-write", 308),
    ("unused-export", 309),
    ("module-name-mismatch", 310),
    ("module-scope-computation", 312),
    ("broken-require", 501),
    ("dynamic-require", 502),
    ("require-outside-project", 503),
    ("stub-never-loaded", 506),
    ("enumeration-require", 508),
    ("circular-require", 509),
    ("almost-swapped", 601),
    ("divide-by-zero", 602),
    ("if-same-then-else", 603),
    ("ifs-same-cond", 604),
    ("suspicious-reverse-loop", 605),
    ("type-check-inside-call", 606),
    ("constant-table-comparison", 607),
    ("unbalanced-assignments", 608),
    ("suspicious-for-step", 609),
    ("compare-nan", 610),
    ("bad-string-escape", 611),
    ("empty-if", 701),
    ("empty-loop", 702),
    ("mixed-table", 703),
    ("manual-table-clone", 704),
    ("parenthesized-condition", 706),
    ("needless-quote-escape", 707),
    ("must-use", 708),
    ("color3-bounds", 801),
    ("player-map-never-cleared", 802),
    ("weak-player-map-never-cleared", 803),
    ("suspicious-udim2-new", 804),
    ("parse-error", 900),
    ("internal-error", 901),
];

/// R0901: Rime itself crashed (a caught panic) while working on a file or a
/// whole-project rule. Not a finding about the checked code, so nothing
/// that silences findings applies to it: rime.toml levels, `--only`,
/// `ignore`, and inline allows all leave it standing, and `rime check`
/// exits with its own code when one was reported.
pub const INTERNAL_ERROR: Code = Code(901);

/// Every core rule's default severity and a one-line summary — what
/// `rime config-template` prints into `examples/rime.toml`. The severity
/// must match what the rule actually constructs its diagnostics with
/// (`Diagnostic::error` vs `Diagnostic::warning`); the test below enforces
/// one entry per `RULE_NAMES` code, nothing enforces the severity itself.
pub const RULE_DEFAULTS: &[(u16, Severity, &str)] = &[
    (11, Severity::Error, "a local shadows an outer binding of the same name"),
    (12, Severity::Error, "the same key is written twice in one table constructor"),
    (13, Severity::Warning, "a local is declared but never read"),
    (14, Severity::Warning, "a parameter is never read (prefix it with `_` to keep it)"),
    (15, Severity::Warning, "a local function is never called"),
    (16, Severity::Warning, "a value is stored into a binding that is never read afterwards"),
    (17, Severity::Warning, "a local is built up but its final value is never used"),
    (21, Severity::Warning, "a `-- rime: allow(...)` directive Rime cannot parse (nothing is suppressed)"),
    (22, Severity::Error, "a read of a name that no scope, std library, or file-level global write defines, or of `...` in a function that does not declare it"),
    (23, Severity::Error, "a write to a name with no `local` in scope (an implicit global)"),
    (24, Severity::Warning, "a use of `_G` or `shared`"),
    (25, Severity::Error, "a rime-rules/ file failed to load, or its check threw"),
    (41, Severity::Error, "a write to a table that was `table.freeze`d"),
    (42, Severity::Error, "`setmetatable` on a frozen table"),
    (43, Severity::Error, "an assignment to a name declared `const`"),
    (44, Severity::Error, "a function attribute Luau does not know (`@foo function`), or one given twice"),
    (101, Severity::Error, "a local may be read before it is assigned"),
    (102, Severity::Error, "a function with a non-nil return type may fall off the end"),
    (103, Severity::Warning, "statements after a return, break, or throw"),
    (104, Severity::Warning, "a stored value is unconditionally overwritten before any read"),
    (110, Severity::Error, "the result of `pcall` is dropped, so failures go unnoticed"),
    (111, Severity::Error, "a table is modified while it is being iterated"),
    (201, Severity::Error, "a value that may be nil is used where nil throws"),
    (202, Severity::Error, "a value is used, then nil-checked afterwards"),
    (301, Severity::Error, "a call omits a parameter whose type excludes nil"),
    (302, Severity::Error, "a literal argument does not match a parameter annotated `number`, `string` or `boolean`"),
    (303, Severity::Error, "`.` used to call a method, or `:` to call a plain function"),
    (304, Severity::Error, "more arguments than the callee declares"),
    (305, Severity::Error, "a module's exported function is reassigned"),
    (306, Severity::Error, "a call to a member the module does not have: a required module's, or the file's own export"),
    (307, Severity::Error, "a read of a field the required module does not export"),
    (308, Severity::Warning, "a write into another module's export table"),
    (309, Severity::Warning, "an exported member no file in the project uses"),
    (310, Severity::Warning, "the module's export local is not named after its file"),
    (312, Severity::Warning, "a call or a loop at a module's top level, which runs when the module is first required"),
    (501, Severity::Error, "a require whose target file does not exist"),
    (502, Severity::Error, "a require Rime cannot resolve statically"),
    (503, Severity::Error, "a require of a path outside the project's mounts"),
    (506, Severity::Error, "a `false and require(...)` stub whose real require never happens"),
    (508, Severity::Error, "`require` over a `GetChildren()`-style enumeration"),
    (509, Severity::Error, "modules that require each other at load time (a require cycle)"),
    (601, Severity::Error, "`a = b` followed by `b = a`"),
    (602, Severity::Warning, "division or modulo by a literal zero"),
    (603, Severity::Error, "an `if` arm identical to its `else`"),
    (604, Severity::Error, "an `elseif` repeating an earlier condition"),
    (605, Severity::Error, "a numeric `for` counting the wrong way (`#t, 1` without `-1`)"),
    (606, Severity::Error, "`type(x == \"number\")` — the comparison is inside the call"),
    (607, Severity::Error, "a comparison against a fresh table literal"),
    (608, Severity::Error, "more names than values, or more values than names, in one assignment"),
    (609, Severity::Error, "a numeric `for` with a step of 0, or a step that contradicts its range"),
    (610, Severity::Error, "`x == 0 / 0` — NaN equals nothing, so the comparison is constant"),
    (611, Severity::Error, "a string escape Luau refuses (`\\300`, `\\xZ`), or a backslash that escapes nothing (`\\d`)"),
    (701, Severity::Warning, "an `if` arm with no statements"),
    (702, Severity::Warning, "a loop body with no statements"),
    (703, Severity::Warning, "a table constructor mixing array items and keyed fields"),
    (704, Severity::Warning, "a loop that copies a table by hand instead of `table.clone`"),
    (706, Severity::Warning, "a condition wholly inside parentheses: `if (x) then`"),
    (707, Severity::Warning, "a backslash before a quote that needs none: `\\'` in a double-quoted string"),
    (708, Severity::Warning, "a call to a library function that only returns a value, with the value dropped: `math.max(a, b)` on its own line"),
    (801, Severity::Warning, "`Color3.new` with a channel outside 0–1"),
    (802, Severity::Warning, "a module table keyed by Player that is never cleared (it holds the Player)"),
    (803, Severity::Warning, "a weak-keyed Player map that is never cleared (it holds its values)"),
    (804, Severity::Warning, "`UDim2.new` with one to three arguments (it takes four)"),
    (900, Severity::Error, "the file could not be parsed as Luau, or uses syntax Rime cannot check yet"),
    (901, Severity::Error, "Rime itself crashed on this file (a bug in Rime, not in the code)"),
];

/// Rules that report nothing until rime.toml gives them a level (or
/// `--only` names them). Each enforces a convention a project has to
/// choose, so it has no sensible "on" for a project that never asked. The
/// severity in `RULE_DEFAULTS` is what `--only` reports them at.
pub const OFF_BY_DEFAULT: &[u16] = &[110, 305, 310, 312, 508, 703];

/// Whether `code` is off until rime.toml gives it a level.
pub fn off_by_default(code: Code) -> bool {
    OFF_BY_DEFAULT.contains(&code.0)
}

/// A core rule's default severity and summary.
pub fn rule_default(code: Code) -> Option<(Severity, &'static str)> {
    RULE_DEFAULTS
        .iter()
        .find(|(c, _, _)| *c == code.0)
        .map(|(_, severity, summary)| (*severity, *summary))
}

#[cfg(test)]
mod defaults_tests {
    use super::*;

    #[test]
    fn every_core_rule_has_exactly_one_default() {
        for (name, code) in RULE_NAMES {
            let n = RULE_DEFAULTS.iter().filter(|(c, _, _)| c == code).count();
            assert_eq!(n, 1, "{name} (R{code:04}) needs exactly one RULE_DEFAULTS entry");
        }
        for (code, _, summary) in RULE_DEFAULTS {
            assert!(RULE_NAMES.iter().any(|(_, c)| c == code), "RULE_DEFAULTS names R{code:04}, which has no rule name");
            assert!(!summary.is_empty() && !summary.ends_with('.'), "R{code:04}: one line, no trailing period");
        }
    }

    #[test]
    fn every_off_by_default_rule_is_a_core_rule() {
        for code in OFF_BY_DEFAULT {
            assert!(rule_default(Code(*code)).is_some(), "R{code:04} is off by default but is no rule");
            assert!(off_by_default(Code(*code)));
        }
        assert!(!off_by_default(Code(22)));
        assert!(!off_by_default(INTERNAL_ERROR), "a crash is never off");
    }
}

/// First code handed to custom project rules (`rime-rules/*.luau`); every
/// core code stays below it. Names are registered at rule-load time and
/// answered by `rule_name`, `allow::resolve_rule` and rime.toml key lookup.
pub const CUSTOM_CODE_BASE: u16 = 9001;

fn custom_registry() -> &'static std::sync::RwLock<Vec<&'static str>> {
    static REG: std::sync::OnceLock<std::sync::RwLock<Vec<&'static str>>> =
        std::sync::OnceLock::new();
    REG.get_or_init(|| std::sync::RwLock::new(Vec::new()))
}

/// Register a custom rule name; returns its code, `R9001` upward in call
/// order. Re-registering a name returns the code it already has. The name
/// is leaked once so `rule_name` can keep returning `&'static str`.
pub fn register_custom(name: &str) -> Code {
    let mut reg = custom_registry().write().expect("custom registry poisoned");
    if let Some(i) = reg.iter().position(|n| *n == name) {
        return Code(CUSTOM_CODE_BASE + i as u16);
    }
    reg.push(Box::leak(name.to_string().into_boxed_str()));
    Code(CUSTOM_CODE_BASE + (reg.len() - 1) as u16)
}

/// The code a custom name was registered under, if any.
pub fn custom_code(name: &str) -> Option<Code> {
    let reg = custom_registry().read().expect("custom registry poisoned");
    reg.iter().position(|n| *n == name).map(|i| Code(CUSTOM_CODE_BASE + i as u16))
}

/// The friendly name for a code, when one exists.
pub fn rule_name(code: Code) -> Option<&'static str> {
    if code.0 >= CUSTOM_CODE_BASE {
        let reg = custom_registry().read().expect("custom registry poisoned");
        return reg.get((code.0 - CUSTOM_CODE_BASE) as usize).copied();
    }
    RULE_NAMES
        .iter()
        .find(|(_, c)| *c == code.0)
        .map(|(n, _)| *n)
}

/// A span with an explanatory message. The primary label is the site of the
/// problem; secondary labels are the other places the reader needs to see
/// (the freeze site, the yield that discarded a narrowing, the write that
/// forced a parameter to be mutable).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub primary: bool,
}

impl Label {
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), primary: true }
    }
    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), primary: false }
    }
}

/// A mechanical fix: replace `span` with `replacement`. Kept structured
/// rather than baked into prose so it can become an LSP code action
/// unchanged (plan §11).
///
/// Editors and agents apply one without reading it, so the result must be
/// the code the message describes. Advice that has no text to put in the
/// file is not a `Suggestion` — it goes in `Diagnostic::help`. (An
/// empty `replacement` is an instruction to delete `span`.)
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub message: String,
    pub span: Span,
    pub replacement: String,
}

/// A line of a file in the project. Not a span, because the file is not
/// always the one the diagnostic is reported in, and a span means nothing
/// without its source.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    /// One line, lowercase, no trailing period. States what happened.
    pub message: String,
    pub labels: Vec<Label>,
    /// Why it is a problem. Rendered as `= note:` lines.
    pub notes: Vec<String>,
    /// What to do about it, in words. Rendered as `= help:` lines. Never
    /// offered as an edit.
    pub help: Vec<String>,
    /// Edits a tool may apply as they are.
    pub suggestions: Vec<Suggestion>,
    /// Set when this diagnostic rejects code that is actually safe.
    ///
    /// Plan §6 rule 7: these must read as a stated limitation of Rime, not as
    /// the user's bug. Also what lets `tests/reject-conservative` be counted
    /// separately as the false-positive budget (plan §8).
    pub conservative: bool,
    /// Where the construct this report is about begins, when the primary
    /// label points further in: the start of a call, for a report on one
    /// of its arguments. An inline `-- rime: allow(...)` that targets the
    /// line of this offset silences the report, as one that targets the
    /// primary label's line does (`allow::apply`). Never rendered.
    pub allow_anchor: Option<usize>,
    /// The whole of the code the diagnostic is about, when a tool needs
    /// more than the labels mark: for an unused export, its definition
    /// from first character to last, where the primary label marks only
    /// the name. Deleting exactly this span removes the thing and leaves
    /// the file parseable. `None` when there is no single such span. For
    /// tools (`--format json`); the text renderer does not show it.
    pub extent: Option<Span>,
    /// Every place the name this diagnostic is about is written as a
    /// string literal, when that weakens the diagnostic: code Rime cannot
    /// follow may reach the thing by that name. The diagnostic still
    /// stands, and a note names the first few places. A tool that acts on
    /// the diagnostic without a person reading it (deleting an unused
    /// export) should skip one where this is not empty.
    pub string_matches: Vec<Site>,
}

impl Diagnostic {
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
            suggestions: Vec::new(),
            conservative: false,
            allow_anchor: None,
            extent: None,
            string_matches: Vec::new(),
        }
    }

    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, ..Self::error(code, message) }
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help.push(help.into());
        self
    }

    pub fn with_suggestion(mut self, s: Suggestion) -> Self {
        self.suggestions.push(s);
        self
    }

    /// Record the whole of the code the diagnostic is about — see `extent`.
    pub fn with_extent(mut self, extent: Span) -> Self {
        self.extent = Some(extent);
        self
    }

    /// Record where the name is also written as a string — see
    /// `string_matches`. The caller adds the note that says so.
    pub fn with_string_matches(mut self, sites: Vec<Site>) -> Self {
        self.string_matches = sites;
        self
    }

    /// Mark as a conservative rejection and record what would make it pass.
    pub fn conservative(mut self, why: impl Into<String>) -> Self {
        self.conservative = true;
        self.notes.push(why.into());
        self
    }

    /// Name where the construct this report is about begins — see
    /// `allow_anchor`.
    pub fn with_allow_anchor(mut self, offset: usize) -> Self {
        self.allow_anchor = Some(offset);
        self
    }

    /// Whether this is R0901 — see `INTERNAL_ERROR`.
    pub fn is_internal_error(&self) -> bool {
        self.code == INTERNAL_ERROR
    }

    pub fn primary_span(&self) -> Span {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span.clone())
            .unwrap_or(0..0)
    }
}

/// Sort into source order and drop diagnostics suppressed by an earlier root
/// cause (plan §6 rule 5: one error per root cause, no cascades).
pub fn finalize(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags.sort_by_key(|d| (d.primary_span().start, d.code));
    diags
}

pub mod allow;
pub mod render;

#[cfg(test)]
mod eq_tests {
    use super::*;

    #[test]
    fn diagnostics_compare_structurally() {
        let a = Diagnostic::error(Code(41), "x").with_label(Label::primary(1..2, "here"));
        let b = Diagnostic::error(Code(41), "x").with_label(Label::primary(1..2, "here"));
        let moved = Diagnostic::error(Code(41), "x").with_label(Label::primary(2..3, "here"));
        assert_eq!(a, b);
        assert_ne!(a, moved, "a shifted span is a different diagnostic");
    }

    #[test]
    fn custom_registry_numbers_in_order_and_is_idempotent() {
        // The registry is process-global and other tests register names on
        // parallel threads, so assert order and identity, never adjacency
        // or absolute numbers.
        let a = register_custom("eq-tests-alpha");
        let b = register_custom("eq-tests-beta");
        assert!(a.0 >= CUSTOM_CODE_BASE);
        assert!(b.0 > a.0, "later registration gets a higher code");
        assert_eq!(register_custom("eq-tests-alpha"), a, "re-registering keeps the code");
        assert_eq!(custom_code("eq-tests-alpha"), Some(a));
        assert_eq!(custom_code("eq-tests-beta"), Some(b));
        assert_eq!(custom_code("eq-tests-never"), None);
    }

    #[test]
    fn rule_name_answers_custom_codes() {
        let c = register_custom("eq-tests-named");
        assert_eq!(rule_name(c), Some("eq-tests-named"));
        assert_eq!(rule_name(Code(25)), Some("custom-rule-error"));
        assert_eq!(rule_name(Code(9999)), None, "unregistered custom slot");
    }
}
