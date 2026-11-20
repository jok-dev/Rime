//! R0310: module naming — the exported local matches its file (decided
//! 2026-08-22). Project mode only: the rule needs the file path. Off until
//! rime.toml gives it a level: which name a module's local carries is a
//! convention a project chooses.
//!
//! The expected name is the file stem with dashes, dots, and spaces
//! stripped (`Catalog-V2.luau` → `CatalogV2`, `Summer Sale Shop NPC.lua` →
//! `SummerSaleShopNPC`); an `init.luau` / `init.lua` takes its PARENT
//! directory's name (the Rojo mount name). Only two export shapes are
//! checked — a root `return <local>` and `return table.freeze(<local>)`;
//! anything else (`setmetatable`, constructors, linkers, anonymous tables,
//! scripts with no return) has no module local to name and stays silent.
//!
//! R0310 reports a module local whose name differs from the expected name.
//! A file name that cannot BE an identifier even after the stripping
//! (`3DModels.luau`) has no expected name, and nothing is reported.
//!
//! A conservative warning: the code is valid Luau rejected on naming
//! convention alone.

use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::path::Path;

/// The name this module's export local should carry, derived from `path`:
/// the file stem with dashes, dots, and spaces stripped (`Catalog-V2` →
/// `CatalogV2`, `Inventory.Telemetry` → `InventoryTelemetry`,
/// `Summer Sale Shop NPC` → `SummerSaleShopNPC`), or the parent
/// directory's name for init files. `None` when the derived name is not a
/// valid identifier: no local can match it.
pub(crate) fn expected_name(path: &Path) -> Option<String> {
    let raw = raw_name(path)?;
    let cleaned: String = raw
        .chars()
        .filter(|&c| c != '-' && c != '.' && c != ' ')
        .collect();
    let mut chars = cleaned.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if head_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(cleaned)
    } else {
        None
    }
}

/// File stem, with init files deferring to their parent directory (the
/// Rojo mount name). `foo.server`/`foo.client` script-class suffixes are
/// stripped first, same as the resolver's `instance_of` — Scripts rarely
/// have a root return, but when one does, `Worker` is the instance name,
/// not `Worker.client`.
fn raw_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let stem = stem
        .strip_suffix(".server")
        .or_else(|| stem.strip_suffix(".client"))
        .unwrap_or(&stem);
    if stem == "init" {
        return Some(path.parent()?.file_name()?.to_string_lossy().into_owned());
    }
    Some(stem.to_string())
}

/// The root-return module local: `return M` or `return table.freeze(M)`,
/// with the span of the name as written at the return site.
fn exported_local(module: &hir::Module) -> Option<(&str, &Span)> {
    let root = module.body(module.root);
    let hir::Terminator::Return { exprs, .. } = root.block.terminator.as_ref()? else {
        return None;
    };
    match exprs.first()? {
        // `return script` exports the module's own INSTANCE (the wally SDK
        // namespace idiom) — there is no module local to name.
        hir::Expr::Name { name, .. } if name == "script" => None,
        hir::Expr::Name { name, span } => Some((name, span)),
        e => {
            super::calls::freeze_wrapped_name(e)?;
            let hir::Expr::Call { args, .. } = e else {
                unreachable!()
            };
            let Some(hir::Expr::Name { name, span }) = args.first() else {
                unreachable!()
            };
            Some((name, span))
        }
    }
}

pub(crate) fn check(module: &hir::Module, path: &Path, diags: &mut Vec<Diagnostic>) {
    let Some((local, span)) = exported_local(module) else {
        return;
    };
    let Some(expected) = expected_name(path) else {
        return;
    };
    if local != expected {
        diags.push(mismatch_diag(local, &expected, span));
    }
}

fn mismatch_diag(local: &str, expected: &str, span: &Span) -> Diagnostic {
    Diagnostic::warning(
        Code(310),
        format!("module local `{local}` should be named `{expected}`"),
    )
    .with_label(Label::primary(
        span.clone(),
        format!("`{local}` returned here, but the file is named `{expected}`"),
    ))
    .conservative(
        "a module named after its file greps and reads the same everywhere \
         it is required; downgrade `module-name-mismatch` in rime.toml if \
         the convention is unwanted",
    )
    // Advice, not an edit: the span is one occurrence of the name, and a
    // rename written there alone leaves the declaration behind.
    .with_help(format!(
        "rename the local to `{expected}` at its declaration and every use"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn expected_name_strips_dashes() {
        assert_eq!(
            expected_name(&PathBuf::from("mods/Catalog-V2.luau")).as_deref(),
            Some("CatalogV2")
        );
    }

    #[test]
    fn expected_name_strips_dots() {
        // Dotted namespacing (`Inventory.Telemetry.lua`) is a real
        // convention; dots sanitize away like dashes.
        assert_eq!(
            expected_name(&PathBuf::from("mods/Inventory.Telemetry.lua")).as_deref(),
            Some("InventoryTelemetry")
        );
    }

    #[test]
    fn expected_name_strips_spaces() {
        // Spaces are legal in Roblox instance names and common in
        // content-ish module trees (`Summer Sale Shop NPC.lua`); they
        // sanitize away like dashes and dots.
        assert_eq!(
            expected_name(&PathBuf::from("mods/Summer Sale Shop NPC.lua")).as_deref(),
            Some("SummerSaleShopNPC")
        );
    }

    #[test]
    fn expected_name_rejects_nonidentifiers() {
        assert_eq!(expected_name(&PathBuf::from("mods/3DModels.luau")), None);
        assert_eq!(expected_name(&PathBuf::from("mods/7 Eleven.luau")), None);
    }

    #[test]
    fn init_takes_parent_directory_name() {
        assert_eq!(
            expected_name(&PathBuf::from("mods/Trace/init.lua")).as_deref(),
            Some("Trace")
        );
    }

    #[test]
    fn mismatched_init_local_is_flagged() {
        let module = rime_syntax::parse("local module = {}\nreturn module\n").unwrap();
        let mut diags = Vec::new();
        check(
            &module,
            &PathBuf::from("mods/Trace/init.lua"),
            &mut diags,
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("`Trace`"), "{}", diags[0].message);
    }

    #[test]
    fn the_rename_is_advice_not_an_edit() {
        // The only span the rule holds is the name at the `return`. Writing
        // the new name there alone leaves `return Trace` reading a global
        // nobody defined, while the declaration and `module.f` keep the old
        // name. A rename has to move every occurrence or none.
        let src = "local module = {}\nfunction module.f() end\nreturn module\n";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check(&module, &PathBuf::from("mods/Trace/init.lua"), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].suggestions.is_empty(), "{:?}", diags[0].suggestions);
        assert!(
            diags[0].help.iter().any(|h| h.contains("rename the local to `Trace`")),
            "{:?}",
            diags[0].help
        );
    }

    #[test]
    fn a_file_name_that_is_no_identifier_reports_nothing() {
        // No local can carry such a name, so there is nothing to compare,
        // and what a project calls its files is not this rule's business.
        let module = rime_syntax::parse("local x = {}\nreturn x\n").unwrap();
        let mut diags = Vec::new();
        check(&module, &PathBuf::from("mods/3DModels.luau"), &mut diags);
        assert_eq!(diags, Vec::new());
    }
}
