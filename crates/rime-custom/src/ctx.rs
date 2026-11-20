//! The `file` and `ctx` tables a rule's `check(file, ctx)` receives, and
//! the sink `ctx:fail` writes into.

use mlua::{Lua, Result, Table, Value};
use rime_diag::Span;
use std::cell::RefCell;
use std::rc::Rc;

/// One `ctx:fail` call.
pub(crate) struct Fail {
    pub span: Span,
    pub message: String,
    pub note: Option<String>,
}

/// `{ path, src, root }`.
pub(crate) fn make_file(lua: &Lua, rel_path: &str, src: &str, root: Table) -> Result<Table> {
    let file = lua.create_table()?;
    file.set("path", rel_path)?;
    file.set("src", src)?;
    file.set("root", root)?;
    Ok(file)
}

/// A node table (uses its `span`) or a `{ start, end }` table. Offsets are
/// arithmetic a rule wrote, so they are clamped into `src` and moved down to
/// char boundaries: the renderers slice `src` by these spans, and a
/// past-the-end or mid-character offset would panic the whole run.
fn span_of(src: &str, v: &Value) -> Result<Span> {
    let bad = || mlua::Error::runtime("ctx:fail expects a node or { start, end }");
    let Value::Table(t) = v else {
        return Err(bad());
    };
    let table = match t.get::<Value>("span")? {
        Value::Table(s) => s,
        _ => t.clone(),
    };
    let start: Option<usize> = table.get("start")?;
    let end: Option<usize> = table.get("end")?;
    let (Some(mut start), Some(mut end)) = (start, end) else {
        return Err(bad());
    };
    if start > end {
        return Err(bad());
    }
    end = end.min(src.len());
    start = start.min(end);
    // Flooring to a boundary is monotone, so `start <= end` survives it.
    while !src.is_char_boundary(end) {
        end -= 1;
    }
    while !src.is_char_boundary(start) {
        start -= 1;
    }
    Ok(start..end)
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

/// `ctx` with `fail(node_or_span, message, note?)` and `line(node_or_span)`.
pub(crate) fn make_ctx(lua: &Lua, src: &str, sink: Rc<RefCell<Vec<Fail>>>) -> Result<Table> {
    let ctx = lua.create_table()?;
    // One owned copy of the source, shared by both closures: `span_of`
    // clamps against it.
    let text: Rc<str> = Rc::from(src);
    let fail_src = Rc::clone(&text);
    let fail = lua.create_function(move |_, (_this, target, message, note): (Table, Value, String, Option<String>)| {
        let span = span_of(&fail_src, &target)?;
        sink.borrow_mut().push(Fail { span, message, note });
        Ok(())
    })?;
    ctx.set("fail", fail)?;
    let line = lua.create_function(move |_, (_this, target): (Table, Value)| {
        let span = span_of(&text, &target)?;
        Ok(line_of(&text, span.start))
    })?;
    ctx.set("line", line)?;
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rule_body: &str, src: &str) -> (Vec<Fail>, std::result::Result<(), mlua::Error>) {
        let module = rime_syntax::parse(src).unwrap();
        let lua = Lua::new();
        crate::stdlib::install(&lua).unwrap();
        let root = crate::bridge::module_tree(&lua, &module).unwrap();
        let file = make_file(&lua, "src/x.lua", src, root).unwrap();
        let sink = Rc::new(RefCell::new(Vec::new()));
        let ctx = make_ctx(&lua, src, sink.clone()).unwrap();
        let check: mlua::Function = lua.load(rule_body).set_name("@rule").eval().unwrap();
        let result = check.call::<()>((file, ctx));
        let fails = sink.take();
        (fails, result)
    }

    #[test]
    fn fail_records_node_span_message_and_note() {
        let (fails, res) = run(
            "return function(file, ctx) for _, c in rime.calls(file.root) do ctx:fail(c, \"no \" .. rime.callee_name(c), \"why\") end end",
            "print(1)\nfoo:bar()\n",
        );
        res.unwrap();
        assert_eq!(fails.len(), 2);
        assert_eq!(fails[0].message, "no print");
        assert_eq!(fails[0].span, 0..8);
        assert_eq!(fails[0].note.as_deref(), Some("why"));
        assert_eq!(fails[1].message, "no foo:bar");
    }

    #[test]
    fn fail_accepts_a_raw_span_and_ctx_line_counts_from_one() {
        let (fails, res) = run(
            "return function(file, ctx) ctx:fail({ start = 9, [\"end\"] = 12 }, \"line \" .. ctx:line({ start = 9, [\"end\"] = 12 })) end",
            "print(1)\nfoo:bar()\n",
        );
        res.unwrap();
        assert_eq!(fails[0].span, 9..12);
        assert_eq!(fails[0].message, "line 2");
    }

    #[test]
    fn file_exposes_path_src_and_root() {
        let (fails, res) = run(
            "return function(file, ctx) assert(file.path == \"src/x.lua\"); assert(file.src:sub(1, 5) == \"print\"); assert(file.root.kind == \"Body\") end",
            "print(1)\n",
        );
        res.unwrap();
        assert!(fails.is_empty());
    }

    #[test]
    fn fail_clamps_spans_to_char_boundaries_and_source_length() {
        let src = "-- é\nprint(1)\n"; // `é` occupies bytes 3..5
        let (fails, res) = run(
            "return function(file, ctx) ctx:fail({ start = 4, [\"end\"] = 4 }, \"mid\") ctx:fail({ start = 2, [\"end\"] = 999 }, \"long\") end",
            src,
        );
        res.unwrap();
        assert_eq!(fails[0].span, 3..3, "a mid-character offset moves down to a boundary");
        assert_eq!(fails[1].span, 2..src.len(), "past the end clamps to the source length");
    }

    #[test]
    fn bad_fail_argument_is_a_lua_error() {
        let (_, res) = run("return function(file, ctx) ctx:fail(42, \"x\") end", "print(1)\n");
        let err = res.unwrap_err();
        assert!(err.to_string().contains("ctx:fail expects a node or { start, end }"), "{err}");
    }
}
