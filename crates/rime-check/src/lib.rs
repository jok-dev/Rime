//! Checker. Driver only — every rule lives in `rules::*`, one module per
//! rule (or rule family); see `rules/mod.rs` for the layout contract.
//!
//! Pipeline:
//!
//!   source -> CST (full-moon, inside rime-syntax)
//!          -> HIR (parser-agnostic, rime-hir)
//!          -> signature pass   [per module, produces the module interface]
//!          -> body pass        [parallel, per function]
//!               |- CFG construction
//!               \- dataflow (assignment, return, narrowing, yields, freeze)
//!          -> diagnostics
//!
//! Parser firewall: this crate depends on rime-hir and rime-syntax's entry
//! point only — no parser type crosses into any analysis. Swapping the
//! parser is a rime-syntax rewrite, nothing here.
//!
//! The signature/body split is the architecture: because annotations are
//! mandatory, signature extraction never looks inside a body, so bodies check
//! independently and in parallel.

pub(crate) mod capture;
pub mod cfg;
pub mod dataflow;
pub mod definition;
pub mod edits;
pub(crate) mod eval;
pub mod guard;
pub mod hover;
pub mod project;
pub mod rules;
pub(crate) mod scope;
pub mod session;
pub mod timings;
pub mod walk;

use rime_diag::{Code, Diagnostic};
use rime_hir as hir;

/// What rime.toml says to the rules that take settings of their own. The
/// default is a run with no rime.toml. Levels are not here: turning a
/// report into an error, a warning or nothing is the caller's business
/// (rime-cli's `Config::apply`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleOptions {
    /// R0312 module-scope-computation. `None`: the rule does not run — it
    /// is off until a project turns it on.
    pub module_scope: Option<rules::module_scope::Options>,
    /// R0802 / R0803, the player-map rules: the variable names that mean
    /// "a Player".
    pub retention: rules::retention::Options,
}

/// Check a single source string against the bundled Luau+Roblox std set.
/// Returns diagnostics in source order.
pub fn check_source(src: &str) -> Vec<Diagnostic> {
    check_source_with(src, rime_std::StdNames::bundled())
}

/// `check_source` against a caller-supplied std set (rime.toml's `[std]`,
/// via `Config::build_std_names`). Returns diagnostics in source order. A
/// crash inside Rime comes back as the one R0901 (see `guard`), never as a
/// panic out of this call.
pub fn check_source_with(src: &str, std: &rime_std::StdNames) -> Vec<Diagnostic> {
    check_source_configured(src, None, std, &RuleOptions::default())
}

/// `check_source_with` under a project's rule settings. `path` is the
/// file's, for the rules that read a file's name; `None` when the source
/// has none.
pub fn check_source_configured(
    src: &str,
    path: Option<&std::path::Path>,
    std: &rime_std::StdNames,
    options: &RuleOptions,
) -> Vec<Diagnostic> {
    guard::catch(|| check_source_unguarded(src, path, std, options)).unwrap_or_else(|crash| {
        vec![crash.diagnostic("checking", "none of this file's findings are known")]
    })
}

fn check_source_unguarded(
    src: &str,
    path: Option<&std::path::Path>,
    std: &rime_std::StdNames,
    options: &RuleOptions,
) -> Vec<Diagnostic> {
    let module = match guard::phase("parse", || rime_syntax::parse(src)) {
        Ok(module) => module,
        Err(mut errors) => {
            // One error per root cause (plan §6 rule 5): report the first
            // parse error, not the cascade behind it. Styled like every
            // other diagnostic: reason in the message, label at the token,
            // parser detail as a note.
            let mut d = match errors.first_mut() {
                Some(e) => {
                    // `unsupported`: the parser read the file and the
                    // lowering refused a construct (rime-syntax
                    // `condition_binding`). Nothing failed to parse, so
                    // neither the message nor the label says so.
                    let (lead, label) = if e.unsupported {
                        ("cannot check this file", "used here")
                    } else {
                        ("could not parse this file as Luau", "parsing stopped here")
                    };
                    let mut d = Diagnostic::error(Code(900), format!("{lead}: {}", e.message));
                    if let Some(span) = e.span.clone() {
                        d = d.with_label(rime_diag::Label::primary(span, label));
                    }
                    // The block a missing `end` belongs to — the parser
                    // only knows where it gave up (usually end of file),
                    // rime-syntax's token matcher knows what is still open.
                    if let Some(opener) = e.opener.take() {
                        d = d.with_label(rime_diag::Label::secondary(
                            opener.span,
                            format!("this `{}` is never closed", opener.kind),
                        ));
                    }
                    // Indentation's opinion on WHICH block inside it is
                    // really missing its closer (rime-syntax `IndentHint`).
                    // Its note goes after the parser's own, below.
                    let mut indent_note = None;
                    if let Some(h) = e.hint.take() {
                        let line = |at: usize| src[..at].matches('\n').count() + 1;
                        let closer_word = if h.culprit.kind == "repeat" { "until" } else { "end" };
                        let took = match &h.lines_up_with {
                            Some(o) => format!(
                                "this `{}` lines up with the `{}` on line {}, but closed the `{}` on line {}",
                                h.closer.kind, o.kind, line(o.span.start), h.culprit.kind, line(h.culprit.span.start)
                            ),
                            None => format!(
                                "this `{}` is indented shallower than the `{}` on line {} it closed",
                                h.closer.kind, h.culprit.kind, line(h.culprit.span.start)
                            ),
                        };
                        d = d
                            .with_label(rime_diag::Label::secondary(
                                h.culprit.span,
                                format!("this `{}` is probably missing its `{closer_word}`", h.culprit.kind),
                            ))
                            .with_label(rime_diag::Label::secondary(h.closer.span, took));
                        indent_note = Some(
                            "indentation is a hint, not syntax: the first `end` shallower than \
                             the block it closes is where the pairing went wrong",
                        );
                    }
                    if let Some(note) = e.note.take() {
                        d = d.with_note(note);
                    }
                    if let Some(note) = indent_note {
                        d = d.with_note(note);
                    }
                    d
                }
                None => Diagnostic::error(
                    Code(900),
                    "could not parse this file as Luau: unknown parse error",
                ),
            };
            d.notes.push("none of this file was checked".to_string());
            return rime_diag::finalize(vec![d]);
        }
    };

    let mut diags = Vec::new();
    for body in &module.bodies {
        check_body(&module, body, src, &mut diags, &mut None);
    }
    let names = scope::resolve(&module);
    rules::calls::check_module(&module, &names, &mut diags);
    rules::must_use::check_module(&module, &names, &mut diags);
    rules::globals::check_module(&module, std, &mut diags);
    rules::retention::check_module(&module, src, &options.retention, &mut diags);
    if let Some(options) = &options.module_scope {
        rules::module_scope::check(&module, &names, path, src, options, &mut diags);
    }
    // Inline `-- rime: allow(...)` suppression. The count of dropped
    // diagnostics is only surfaced in project mode (FileReport) — this
    // signature predates it.
    rime_diag::finalize(rime_diag::allow::apply(src, diags).kept)
}

pub(crate) fn check_body(
    module: &hir::Module,
    body: &hir::Body,
    src: &str,
    diags: &mut Vec<Diagnostic>,
    timings: &mut Option<&mut timings::Timings>,
) {
    use timings::time_opt;

    let cfg = time_opt(timings, "cfg build", || cfg::Builder::build(module, body));

    time_opt(timings, "rule: definite", || rules::definite::check(&cfg, body, diags));
    time_opt(timings, "rule: freeze", || rules::freeze::check(&cfg, src, diags));
    time_opt(timings, "rule: effects", || rules::effects::check(&cfg, diags));
    time_opt(timings, "rule: shadowing", || rules::shadowing::check(&cfg, diags));
    time_opt(timings, "rule: unused", || rules::unused::check(&cfg, module, body, diags));
    // Faint runs before dead-store: a faint local's guarded fallback store
    // would otherwise draw a misleading R0104 (one defect, one diagnostic —
    // the R0017 at the declaration owns it).
    let faint = time_opt(timings, "rule: faint", || {
        rules::faint::check(module, body, diags)
    });
    time_opt(timings, "rule: dead-store", || {
        rules::dead_store::check(&cfg, module, body, &faint, diags)
    });
    time_opt(timings, "rule: duplicate-key", || rules::duplicate_key::check(body, diags));
    time_opt(timings, "rule: suspicious", || rules::suspicious::check(body, src, diags));
    time_opt(timings, "rule: style", || rules::style::check(body, src, diags));
    time_opt(timings, "rule: roblox", || rules::roblox::check(body, src, diags));
    time_opt(timings, "rule: narrowing", || rules::narrowing::check(&cfg, module, body, diags));
    time_opt(timings, "rule: varargs", || rules::globals::check_varargs(body, diags));
    time_opt(timings, "rule: attributes", || rules::attributes::check(body, diags));
    time_opt(timings, "rule: strings", || rules::strings::check(body, src, diags));
}
