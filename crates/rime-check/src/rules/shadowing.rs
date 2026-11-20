//! R0011: shadowing, rejected as bug-prone by default (decided 2026-08-21).
//!
//! Scope decisions that keep it sane: local-shadows-local only (`local
//! pairs = pairs` global caching stays legal), `_`-prefixed names exempt,
//! same-body only (closure-over-outer-local is a documented gap until
//! cross-body scope exists).
//!
//! Detection lives in the CFG builder (`crate::cfg`, `Shadow`): the builder
//! already owns the scope stack, so it records shadowing declarations during
//! its walk rather than a second pass re-implementing scope.

use crate::cfg::{Cfg, Shadow};
use rime_diag::{Code, Diagnostic, Label};

pub fn check(cfg: &Cfg, diags: &mut Vec<Diagnostic>) {
    for s in &cfg.shadows {
        diags.push(diag(cfg, s));
    }
}

/// Conservative — the code is valid Luau; Rime cannot tell a deliberate
/// rebind from a mistake, so it says so and offers the fix.
fn diag(cfg: &Cfg, s: &Shadow) -> Diagnostic {
    let name = &cfg.locals[s.local].name;
    Diagnostic::error(Code(11), format!("`{name}` shadows an earlier declaration"))
        .with_label(Label::primary(
            cfg.locals[s.local].decl_span.clone(),
            format!("`{name}` declared again here"),
        ))
        .with_label(Label::secondary(
            cfg.locals[s.prev].decl_span.clone(),
            "still in scope from here",
        ))
        .conservative(
            "both bindings exist; later reads silently pick the inner one, and Rime \
             cannot tell a deliberate rebind from a mistake",
        )
        .with_help(format!(
            "rename one `{name}`, or prefix with `_` if the outer one is finished"
        ))
}
