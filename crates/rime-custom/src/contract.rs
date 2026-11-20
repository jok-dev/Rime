//! The rule-file contract: what a `rime-rules/*.luau` chunk
//! must return, validated once at load. Every violation is an R0025 on
//! the rule file; the rule is skipped and the others still load.

use globset::{GlobSet, GlobSetBuilder};
use mlua::{Function, Lua, Value};
use rime_diag::{Code, Diagnostic, Label, Severity, Span};
use std::path::{Path, PathBuf};

/// One loaded rule.
#[derive(Debug)]
pub(crate) struct Rule {
    pub id: String,
    pub code: Code,
    pub severity: Severity,
    pub globs: GlobSet,
    pub check: Function,
    pub path: PathBuf,
    pub src: String,
}

/// R0025 anchored on a rule file.
pub(crate) fn error_diag(stem: &str, reason: &str, span: Span) -> Diagnostic {
    Diagnostic::error(Code(25), format!("custom rule `{stem}`: {reason}"))
        .with_label(Label::primary(span, "in this rule file"))
        .with_note("fix the rule file; see crates/rime-custom/lua/rime.luau for the API".to_string())
}

/// The first `:<line>:` in a Lua error message — Luau formats runtime and
/// syntax errors as `<chunk>:<line>: <msg>`.
pub(crate) fn lua_error_line(err: &mlua::Error) -> Option<usize> {
    let text = err.to_string();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b':' {
                return text[i + 1..j].parse().ok();
            }
        }
        i += 1;
    }
    None
}

/// Byte span of the 1-based `line`, or `0..0` when out of range.
pub(crate) fn line_span(src: &str, line: usize) -> Span {
    let mut start = 0usize;
    for (n, l) in src.split_inclusive('\n').enumerate() {
        if n + 1 == line {
            let end = start + l.trim_end_matches(['\n', '\r']).len();
            return start..end;
        }
        start += l.len();
    }
    0..0
}

fn stem_of(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "rule".to_string())
}

pub(crate) fn looks_like_core_code(id: &str) -> bool {
    id.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// A core rule's rime.toml name (`unused-local`, `possible-nil-use`, ...).
/// Reserved for the same reason `R####` is: both are keys `[rules]` already
/// resolves, and a custom rule answering to one would silently retarget
/// every existing config and `-- rime: allow(...)` that names it.
pub(crate) fn is_core_name(id: &str) -> bool {
    rime_diag::RULE_NAMES.iter().any(|(name, _)| *name == id)
}

/// Evaluate one rule file and validate what it returns.
pub(crate) fn load_rule(lua: &Lua, path: &Path, src: &str, existing: &[Rule]) -> Result<Rule, Diagnostic> {
    let stem = stem_of(path);
    let whole = 0..src.len();
    let fail = |reason: String, span: Span| error_diag(&stem, &reason, span);

    let chunk_name = format!("@{}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let value: Value = lua.load(src).set_name(chunk_name).eval().map_err(|e| {
        let span = lua_error_line(&e).map(|l| line_span(src, l)).unwrap_or(whole.clone());
        let kind = if matches!(e, mlua::Error::SyntaxError { .. }) { "syntax error" } else { "error while loading" };
        fail(format!("{kind}: {e}"), span)
    })?;
    let Value::Table(table) = value else {
        return Err(fail("the rule file must return a table".to_string(), whole));
    };

    let id = match table.get::<Value>("id").map_err(|e| fail(e.to_string(), whole.clone()))? {
        Value::Nil => stem.clone(),
        Value::String(s) => s.to_str().map_err(|e| fail(e.to_string(), whole.clone()))?.to_string(),
        _ => return Err(fail("`id` must be a string".to_string(), whole)),
    };
    if looks_like_core_code(&id) {
        return Err(fail(format!("id `{id}` looks like a core code; `R####` ids are reserved"), whole));
    }
    if is_core_name(&id) {
        return Err(fail(format!("id `{id}` is a core rule name; pick another"), whole));
    }
    if existing.iter().any(|r| r.id == id) {
        return Err(fail(format!("duplicate rule id `{id}`"), whole));
    }

    let severity = match table.get::<Value>("severity").map_err(|e| fail(e.to_string(), whole.clone()))? {
        Value::Nil => Severity::Error,
        Value::String(s) => match s.to_str().map_err(|e| fail(e.to_string(), whole.clone()))?.as_ref() {
            "error" => Severity::Error,
            "warn" => Severity::Warning,
            other => return Err(fail(format!("`severity` must be \"error\" or \"warn\", got \"{other}\""), whole)),
        },
        _ => return Err(fail("`severity` must be \"error\" or \"warn\"".to_string(), whole)),
    };

    if table.contains_key("check_project").map_err(|e| fail(e.to_string(), whole.clone()))? {
        return Err(fail("check_project is not supported yet (per-file `check` only)".to_string(), whole));
    }
    let check: Function = match table.get::<Value>("check").map_err(|e| fail(e.to_string(), whole.clone()))? {
        Value::Function(f) => f,
        Value::Nil => return Err(fail("missing `check` function".to_string(), whole)),
        _ => return Err(fail("`check` must be a function".to_string(), whole)),
    };

    let patterns: Vec<String> = match table.get::<Value>("files").map_err(|e| fail(e.to_string(), whole.clone()))? {
        Value::String(s) => vec![s.to_str().map_err(|e| fail(e.to_string(), whole.clone()))?.to_string()],
        Value::Table(list) => {
            let mut out = Vec::new();
            for item in list.sequence_values::<String>() {
                out.push(item.map_err(|_| fail("`files` list must hold strings".to_string(), whole.clone()))?);
            }
            if out.is_empty() {
                return Err(fail("`files` list is empty".to_string(), whole));
            }
            out
        }
        Value::Nil => return Err(fail("missing `files` (a glob or list of globs)".to_string(), whole)),
        _ => return Err(fail("`files` must be a glob string or a list of them".to_string(), whole)),
    };
    let mut builder = GlobSetBuilder::new();
    for p in &patterns {
        let glob = globset::GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .map_err(|e| fail(format!("invalid `files` pattern `{p}`: {e}"), whole.clone()))?;
        builder.add(glob);
    }
    let globs = builder.build().map_err(|e| fail(format!("invalid `files`: {e}"), whole))?;

    let code = rime_diag::register_custom(&id);
    Ok(Rule { id, code, severity, globs, check, path: path.to_path_buf(), src: src.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        crate::stdlib::install(&lua).unwrap();
        lua
    }

    fn load(src: &str) -> Result<Rule, Diagnostic> {
        load_rule(&lua(), Path::new("rime-rules/my-rule.luau"), src, &[])
    }

    #[test]
    fn minimal_rule_loads_with_defaults() {
        let r = load("return { files = \"src/**/*.lua\", check = function(file, ctx) end }").unwrap();
        assert_eq!(r.id, "my-rule");
        assert_eq!(r.severity, Severity::Error);
        assert_eq!(r.code, rime_diag::custom_code("my-rule").unwrap());
        assert!(r.globs.is_match("src/a/b.lua"));
        assert!(!r.globs.is_match("src/a/b.luau"));
    }

    #[test]
    fn explicit_id_severity_and_file_list() {
        let r = load("return { id = \"contract-tests-explicit\", severity = \"warn\", files = { \"*.lua\", \"lib/**\" }, check = function() end }").unwrap();
        assert_eq!(r.id, "contract-tests-explicit");
        assert_eq!(r.severity, Severity::Warning);
        assert!(r.globs.is_match("x.lua") && r.globs.is_match("lib/deep/y.luau"));
        assert!(!r.globs.is_match("src/x.lua"), "`*` does not cross `/`");
    }

    #[test]
    fn rejections_name_the_field() {
        let cases: &[(&str, &str)] = &[
            ("return 1", "must return a table"),
            ("return { check = function() end }", "`files`"),
            ("return { files = \"*.lua\" }", "`check`"),
            ("return { files = \"*.lua\", check = 1 }", "`check`"),
            ("return { files = \"*.lua\", check = function() end, severity = \"loud\" }", "`severity`"),
            ("return { files = \"*.lua\", check = function() end, check_project = function() end }", "check_project is not supported yet"),
            ("return { id = \"R0001\", files = \"*.lua\", check = function() end }", "looks like a core code"),
            ("return { id = \"unused-local\", files = \"*.lua\", check = function() end }", "core rule name"),
            ("return { files = \"[\", check = function() end }", "invalid `files` pattern"),
            ("local x = = 1", "syntax"),
        ];
        for (src, needle) in cases {
            let d = load(src).expect_err(src);
            assert_eq!(d.code, Code(25), "{src}");
            assert_eq!(d.severity, Severity::Error);
            assert!(d.message.contains(needle) || d.notes.iter().any(|n| n.contains(needle)), "{src}: {} / {:?}", d.message, d.notes);
            assert!(d.message.starts_with("custom rule `my-rule`"), "{}", d.message);
        }
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let lua = lua();
        let first = load_rule(&lua, Path::new("rime-rules/dup.luau"), "return { files = \"*\", check = function() end }", &[]).unwrap();
        let err = load_rule(&lua, Path::new("rime-rules/other.luau"), "return { id = \"dup\", files = \"*\", check = function() end }", &[first]).unwrap_err();
        assert!(err.message.contains("duplicate"), "{}", err.message);
    }

    #[test]
    fn lua_error_line_and_line_span() {
        let lua = lua();
        let err = lua.load("local a = 1\nerror('boom')\n").set_name("@r.luau").exec().unwrap_err();
        assert_eq!(lua_error_line(&err), Some(2), "{err}");
        assert_eq!(line_span("ab\ncde\nf", 2), 3..6);
        assert_eq!(line_span("ab\ncde\nf", 9), 0..0, "out of range → empty span at 0");
    }
}
