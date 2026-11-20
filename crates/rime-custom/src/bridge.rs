//! HIR → Lua table tree. One conversion per checked file; the tree is
//! dropped after that file's rules run. Every node is `{ kind, span,
//! parent, ...fields }` — see the kind table in lua/rime.luau.

use mlua::{Lua, Result, Table, Value};
use rime_hir as hir;

/// Convert a module into one Lua tree rooted at `kind = "Body"`. Closures
/// are inlined from the body arena so rules see a single tree.
pub fn module_tree(lua: &Lua, module: &hir::Module) -> Result<Table> {
    let b = Bridge { lua, module };
    let root = b.body(module.body(module.root), "Body")?;
    link_and_span(lua, &root)?;
    Ok(root)
}

struct Bridge<'a> {
    lua: &'a Lua,
    module: &'a hir::Module,
}

impl Bridge<'_> {
    fn node(&self, kind: &str, span: Option<&hir::Span>) -> Result<Table> {
        let t = self.lua.create_table()?;
        t.set("kind", kind)?;
        if let Some(s) = span {
            t.set("span", self.span(s)?)?;
        }
        Ok(t)
    }

    fn span(&self, s: &hir::Span) -> Result<Table> {
        let t = self.lua.create_table()?;
        t.set("start", s.start)?;
        t.set("end", s.end)?;
        Ok(t)
    }

    fn list<T>(&self, items: &[T], f: impl Fn(&T) -> Result<Table>) -> Result<Table> {
        let t = self.lua.create_table()?;
        for (i, item) in items.iter().enumerate() {
            t.set(i + 1, f(item)?)?;
        }
        Ok(t)
    }

    fn strings(&self, items: &[String]) -> Result<Table> {
        let t = self.lua.create_table()?;
        for (i, s) in items.iter().enumerate() {
            t.set(i + 1, s.as_str())?;
        }
        Ok(t)
    }

    fn ty(&self, ty: &hir::Ty) -> Result<Table> {
        let t = self.lua.create_table()?;
        t.set("rendered", ty.rendered.as_str())?;
        t.set("admits_nil", ty.admits_nil)?;
        t.set("span", self.span(&ty.span)?)?;
        Ok(t)
    }

    fn decl(&self, d: &hir::LocalDecl, kind: &str) -> Result<Table> {
        let t = self.node(kind, Some(&d.span))?;
        t.set("name", d.name.as_str())?;
        t.set("is_const", d.is_const)?;
        if let Some(ty) = &d.ty {
            t.set("ty", self.ty(ty)?)?;
        }
        Ok(t)
    }

    fn body(&self, body: &hir::Body, kind: &str) -> Result<Table> {
        let t = self.node(kind, None)?;
        t.set("params", self.list(&body.params, |p| self.decl(p, "Param"))?)?;
        t.set("has_varargs", body.has_varargs)?;
        if let Some(rt) = &body.return_ty {
            t.set("return_ty", self.ty(rt)?)?;
        }
        if let Some(s) = &body.end_span {
            t.set("end_span", self.span(s)?)?;
        }
        t.set("block", self.block(&body.block)?)?;
        Ok(t)
    }

    fn closure(&self, id: hir::BodyId) -> Result<Table> {
        self.body(self.module.body(id), "Function")
    }

    fn block(&self, block: &hir::Block) -> Result<Table> {
        let t = self.node("Block", None)?;
        t.set("stmts", self.list(&block.stmts, |s| self.stmt(s))?)?;
        if let Some(term) = &block.terminator {
            let term = match term {
                hir::Terminator::Return { exprs, span } => {
                    let r = self.node("Return", Some(span))?;
                    r.set("exprs", self.list(exprs, |e| self.expr(e))?)?;
                    r
                }
                hir::Terminator::Break => self.node("Break", None)?,
                hir::Terminator::Continue => self.node("Continue", None)?,
            };
            t.set("terminator", term)?;
        }
        Ok(t)
    }

    fn target(&self, target: &hir::Target) -> Result<Table> {
        match target {
            hir::Target::Name { name, span } => {
                let t = self.node("Name", Some(span))?;
                t.set("name", name.as_str())?;
                Ok(t)
            }
            hir::Target::Projection(e) => self.expr(e),
        }
    }

    fn stmt(&self, stmt: &hir::Stmt) -> Result<Table> {
        use hir::StmtKind as K;
        let span = Some(&stmt.span);
        let t = match &stmt.kind {
            K::Local { decls, init } => {
                let t = self.node("Local", span)?;
                t.set("decls", self.list(decls, |d| self.decl(d, "Decl"))?)?;
                t.set("init", self.list(init, |e| self.expr(e))?)?;
                t
            }
            K::Assign { targets, values } => {
                let t = self.node("Assign", span)?;
                t.set("targets", self.list(targets, |x| self.target(x))?)?;
                t.set("values", self.list(values, |e| self.expr(e))?)?;
                t
            }
            K::CompoundAssign { target, value } => {
                let t = self.node("CompoundAssign", span)?;
                t.set("target", self.target(target)?)?;
                t.set("value", self.expr(value)?)?;
                t
            }
            K::Call(e) => {
                // The call expression itself sits in `stmts`.
                let t = self.expr(e)?;
                if t.get::<Value>("span")? == Value::Nil {
                    t.set("span", self.span(&stmt.span)?)?;
                }
                t
            }
            K::Do(b) => {
                let t = self.node("Do", span)?;
                t.set("body", self.block(b)?)?;
                t
            }
            K::If { cond, then, else_ifs, or_else } => {
                let t = self.node("If", span)?;
                t.set("cond", self.expr(cond)?)?;
                t.set("then", self.block(then)?)?;
                t.set(
                    "else_ifs",
                    self.list(else_ifs, |(c, b)| {
                        let arm = self.node("ElseIf", None)?;
                        arm.set("cond", self.expr(c)?)?;
                        arm.set("block", self.block(b)?)?;
                        Ok(arm)
                    })?,
                )?;
                if let Some(b) = or_else {
                    t.set("or_else", self.block(b)?)?;
                }
                t
            }
            K::While { cond, body } => {
                let t = self.node("While", span)?;
                t.set("cond", self.expr(cond)?)?;
                t.set("body", self.block(body)?)?;
                t
            }
            K::Repeat { body, until } => {
                let t = self.node("Repeat", span)?;
                t.set("body", self.block(body)?)?;
                t.set("until_", self.expr(until)?)?;
                t
            }
            K::NumericFor { var, start, end, step, body } => {
                let t = self.node("NumericFor", span)?;
                t.set("var", self.decl(var, "Decl")?)?;
                t.set("start", self.expr(start)?)?;
                t.set("end_", self.expr(end)?)?;
                if let Some(s) = step {
                    t.set("step", self.expr(s)?)?;
                }
                t.set("body", self.block(body)?)?;
                t
            }
            K::GenericFor { vars, exprs, body } => {
                let t = self.node("GenericFor", span)?;
                t.set("vars", self.list(vars, |d| self.decl(d, "Decl"))?)?;
                t.set("exprs", self.list(exprs, |e| self.expr(e))?)?;
                t.set("body", self.block(body)?)?;
                t
            }
            K::LocalFunction { decl, body } => {
                let t = self.node("LocalFunction", span)?;
                t.set("decl", self.decl(decl, "Decl")?)?;
                t.set("body", self.closure(*body)?)?;
                t
            }
            K::FunctionDecl { root, path, is_method, assigns_root, body } => {
                let t = self.node("FunctionDecl", span)?;
                t.set("root", root.as_str())?;
                t.set("path", self.strings(path)?)?;
                t.set("is_method", *is_method)?;
                t.set("assigns_root", *assigns_root)?;
                t.set("body", self.closure(*body)?)?;
                t
            }
        };
        Ok(t)
    }

    fn expr(&self, e: &hir::Expr) -> Result<Table> {
        use hir::Expr as E;
        let t = match e {
            E::Name { name, span } => {
                let t = self.node("Name", Some(span))?;
                t.set("name", name.as_str())?;
                t
            }
            E::Call { base, args, method, span, .. } => {
                let t = self.node("Call", Some(span))?;
                t.set("base", self.expr(base)?)?;
                t.set("args", self.list(args, |a| self.expr(a))?)?;
                if let Some(m) = method {
                    t.set("method", m.as_str())?;
                }
                t
            }
            E::Index { base, key, name, name_span } => {
                let t = self.node("Index", None)?;
                t.set("base", self.expr(base)?)?;
                if let Some(k) = key {
                    t.set("key", self.expr(k)?)?;
                }
                if let Some(n) = name {
                    t.set("name", n.as_str())?;
                }
                if let Some(s) = name_span {
                    t.set("name_span", self.span(s)?)?;
                }
                t
            }
            E::Function(id) => self.closure(*id)?,
            E::BinOp { op, lhs, rhs } => {
                let t = self.node("BinOp", None)?;
                t.set("op", format!("{op:?}"))?;
                t.set("lhs", self.expr(lhs)?)?;
                t.set("rhs", self.expr(rhs)?)?;
                t
            }
            E::Not(inner) => {
                let t = self.node("Not", None)?;
                t.set("operand", self.expr(inner)?)?;
                t
            }
            E::UnOp { op, operand, span } => {
                let t = self.node("UnOp", Some(span))?;
                t.set("op", format!("{op:?}"))?;
                t.set("operand", self.expr(operand)?)?;
                t
            }
            E::Table(fields) => {
                let t = self.node("Table", None)?;
                t.set(
                    "fields",
                    self.list(fields, |f| {
                        let node = self.node("Field", None)?;
                        match f {
                            hir::TableField::Named { name, span, value } => {
                                node.set("name", name.as_str())?;
                                node.set("name_span", self.span(span)?)?;
                                node.set("value", self.expr(value)?)?;
                            }
                            hir::TableField::Keyed { key, value } => {
                                node.set("key", self.expr(key)?)?;
                                node.set("value", self.expr(value)?)?;
                            }
                            hir::TableField::Item(e) => node.set("value", self.expr(e)?)?,
                        }
                        Ok(node)
                    })?,
                )?;
                t
            }
            E::IfExpr(arms) => {
                let t = self.node("IfExpr", None)?;
                t.set(
                    "arms",
                    self.list(arms, |(cond, val)| {
                        let arm = self.node("Arm", None)?;
                        if let Some(c) = cond {
                            arm.set("cond", self.expr(c)?)?;
                        }
                        arm.set("value", self.expr(val)?)?;
                        Ok(arm)
                    })?,
                )?;
                t
            }
            E::Composite(parts) => {
                let t = self.node("Composite", None)?;
                t.set("parts", self.list(parts, |p| self.expr(p))?)?;
                t
            }
            E::Literal { prim, span, text } => {
                let t = self.node("Literal", Some(span))?;
                t.set("prim", format!("{prim:?}"))?;
                if let Some(s) = text {
                    t.set("text", s.as_str())?;
                }
                t
            }
            E::Leaf => self.node("Leaf", None)?,
        };
        Ok(t)
    }
}

fn is_node(v: &Value) -> bool {
    matches!(v, Value::Table(t) if matches!(t.get::<Value>("kind"), Ok(Value::String(_))))
}

/// Child nodes of `node`: node-valued fields and node elements of list
/// fields, skipping `parent` and `span`.
fn children(node: &Table) -> Result<Vec<Table>> {
    let mut out = Vec::new();
    for pair in node.clone().pairs::<Value, Value>() {
        let (k, v) = pair?;
        if let Value::String(key) = &k {
            let key = key.to_str()?;
            if key == "parent" || key == "span" {
                continue;
            }
        }
        if is_node(&v) {
            if let Value::Table(t) = v {
                out.push(t);
            }
        } else if let Value::Table(list) = v {
            for item in list.sequence_values::<Value>() {
                let item = item?;
                if is_node(&item) {
                    if let Value::Table(t) = item {
                        out.push(t);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn span_of(node: &Table) -> Result<Option<(usize, usize)>> {
    match node.get::<Value>("span")? {
        Value::Table(s) => Ok(Some((s.get("start")?, s.get("end")?))),
        _ => Ok(None),
    }
}

fn set_span(lua: &Lua, node: &Table, start: usize, end: usize) -> Result<()> {
    let s = lua.create_table()?;
    s.set("start", start)?;
    s.set("end", end)?;
    node.set("span", s)
}

/// Pass 1 (bottom-up): every child gets `parent`, and a node's own span is
/// MERGED with the union of its children's — a span-less node (a `BinOp`,
/// the root `Body`) takes that union outright. The merge, rather than
/// "keep the HIR span", exists for `Call`: its HIR span covers only the
/// call suffix (`(args)` / `:m(args)`), so without it the callee would sit
/// outside its own call node.
/// Pass 2 (top-down): a node still without a span (a `Leaf`, an empty
/// `Block`) takes its parent's; the root, span-less and childless in an
/// empty file, falls back to `(0, 0)`.
fn link_and_span(lua: &Lua, root: &Table) -> Result<()> {
    fn up(lua: &Lua, node: &Table, parent: Option<&Table>) -> Result<Option<(usize, usize)>> {
        if let Some(p) = parent {
            node.set("parent", p.clone())?;
        }
        let mut union: Option<(usize, usize)> = None;
        for child in children(node)? {
            if let Some((s, e)) = up(lua, &child, Some(node))? {
                union = Some(match union {
                    None => (s, e),
                    Some((us, ue)) => (us.min(s), ue.max(e)),
                });
            }
        }
        match span_of(node)? {
            Some(own) => {
                // A node's own span normally contains its children; `Call` is
                // the exception — the HIR keeps only the call suffix
                // (`(args)` / `:m(args)`) — so merge with the children's
                // union and the whole call is covered.
                let merged = match union {
                    Some((s, e)) => (own.0.min(s), own.1.max(e)),
                    None => own,
                };
                if merged != own {
                    set_span(lua, node, merged.0, merged.1)?;
                }
                Ok(Some(merged))
            }
            None => {
                if let Some((s, e)) = union {
                    set_span(lua, node, s, e)?;
                }
                Ok(union)
            }
        }
    }
    fn down(lua: &Lua, node: &Table, inherited: Option<(usize, usize)>) -> Result<()> {
        let own = match span_of(node)? {
            Some(s) => Some(s),
            None => {
                if let Some((s, e)) = inherited {
                    set_span(lua, node, s, e)?;
                }
                inherited
            }
        };
        for child in children(node)? {
            down(lua, &child, own)?;
        }
        Ok(())
    }
    up(lua, root, None)?;
    down(lua, root, Some((0, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_of(src: &str) -> (Lua, Table) {
        let module = rime_syntax::parse(src).expect("fixture parses");
        let lua = Lua::new();
        let root = module_tree(&lua, &module).expect("bridge");
        (lua, root)
    }

    /// Run Lua assertions against the tree bound as global `t`.
    fn check(src: &str, lua_asserts: &str) {
        let (lua, root) = tree_of(src);
        lua.globals().set("t", root).unwrap();
        lua.load(lua_asserts).set_name("@asserts").exec().unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn root_is_a_body_with_a_block() {
        check(
            "local x = 1\n",
            r#"
            assert(t.kind == "Body", t.kind)
            assert(t.block.kind == "Block")
            assert(#t.block.stmts == 1)
            assert(t.parent == nil)
            "#,
        );
    }

    #[test]
    fn statements_keep_hir_fields() {
        check(
            "local a: number, b = 1, f()\na, t.x = 2, 3\na += 1\nprint(a)\ndo end\nif a then elseif b then else end\nwhile a do end\nrepeat until a\nfor i = 1, 10, 2 do end\nfor k, v in pairs(t) do end\nlocal function g() end\nfunction M.h(self) end\nreturn a\n",
            r#"
            local s = t.block.stmts
            assert(s[1].kind == "Local" and s[1].decls[1].name == "a" and s[1].decls[1].ty.rendered == "number" and s[1].decls[2].ty == nil)
            assert(s[1].init[2].kind == "Call")
            assert(s[2].kind == "Assign" and s[2].targets[1].kind == "Name" and s[2].targets[2].kind == "Index" and s[2].targets[2].name == "x")
            assert(s[3].kind == "CompoundAssign" and s[3].target.name == "a" and s[3].value.kind == "Literal")
            assert(s[4].kind == "Call" and s[4].base.name == "print")
            assert(s[5].kind == "Do" and s[5].body.kind == "Block")
            assert(s[6].kind == "If" and s[6].cond.kind == "Name" and s[6].else_ifs[1].kind == "ElseIf" and s[6].or_else.kind == "Block")
            assert(s[7].kind == "While" and s[7].body.kind == "Block")
            assert(s[8].kind == "Repeat" and s[8].until_.kind == "Name")
            assert(s[9].kind == "NumericFor" and s[9].var.name == "i" and s[9].start.text == "1" and s[9].end_.text == "10" and s[9].step.text == "2")
            assert(s[10].kind == "GenericFor" and s[10].vars[2].name == "v" and s[10].exprs[1].kind == "Call")
            assert(s[11].kind == "LocalFunction" and s[11].decl.name == "g" and s[11].body.kind == "Function")
            assert(s[12].kind == "FunctionDecl" and s[12].root == "M" and s[12].path[1] == "h" and s[12].is_method == false and s[12].body.params[1].name == "self")
            assert(t.block.terminator.kind == "Return" and t.block.terminator.exprs[1].name == "a")
            "#,
        );
    }

    #[test]
    fn expressions_keep_hir_fields() {
        check(
            "local v = a.b:c(1, \"s\") + -#x or not y\nlocal w = { n = 1, [2] = 3, 4 }\nlocal z = if a then 1 else nil\nlocal f = function(p, ...) end\n",
            r#"
            local s = t.block.stmts
            local e = s[1].init[1]
            assert(e.kind == "BinOp" and e.op == "Or")
            local plus = e.lhs
            assert(plus.kind == "BinOp" and plus.op == "Add")
            local call = plus.lhs
            assert(call.kind == "Call" and call.method == "c" and #call.args == 2)
            assert(call.base.kind == "Index" and call.base.name == "b" and call.base.base.name == "a")
            assert(call.args[2].kind == "Literal" and call.args[2].prim == "String" and call.args[2].text == "s")
            local neg = plus.rhs
            assert(neg.kind == "UnOp" and neg.op == "Neg" and neg.operand.kind == "UnOp" and neg.operand.op == "Len")
            assert(e.rhs.kind == "Not" and e.rhs.operand.name == "y")
            local tbl = s[2].init[1]
            assert(tbl.kind == "Table" and #tbl.fields == 3)
            assert(tbl.fields[1].kind == "Field" and tbl.fields[1].name == "n")
            assert(tbl.fields[2].key.text == "2" and tbl.fields[3].value.text == "4")
            local ife = s[3].init[1]
            assert(ife.kind == "IfExpr" and ife.arms[1].kind == "Arm" and ife.arms[1].cond.name == "a" and ife.arms[2].cond == nil and ife.arms[2].value.prim == "Nil")
            local fn = s[4].init[1]
            assert(fn.kind == "Function" and fn.params[1].kind == "Param" and fn.params[1].name == "p" and fn.has_varargs == true and fn.block.kind == "Block")
            "#,
        );
    }

    #[test]
    fn every_node_has_a_span_and_children_link_to_parents() {
        check(
            "local x = a.b + 1\nif x then print(x) end\n",
            r#"
            local function visit(n, parent)
                assert(type(n.span) == "table" and type(n.span.start) == "number" and type(n.span["end"]) == "number", n.kind .. " lacks a span")
                assert(n.span.start <= n.span["end"], n.kind)
                assert(n.parent == parent, n.kind .. " parent link")
                for k, v in pairs(n) do
                    if k ~= "parent" and k ~= "span" and type(v) == "table" then
                        if type(v.kind) == "string" then visit(v, n)
                        else for _, item in ipairs(v) do if type(item) == "table" and type(item.kind) == "string" then visit(item, n) end end end
                    end
                end
            end
            visit(t, nil)
            -- a BinOp has no HIR span of its own: it gets the union of its operands
            local bin = t.block.stmts[1].init[1]
            assert(bin.kind == "BinOp")
            assert(bin.span.start == bin.lhs.span.start and bin.span["end"] == bin.rhs.span["end"])
            "#,
        );
    }

    #[test]
    fn call_span_covers_the_callee_not_just_the_arguments() {
        check(
            "print(1)\nfoo.bar:baz(2)\n",
            r#"
            local s = t.block.stmts
            assert(s[1].span.start == 0 and s[1].span["end"] == 8, s[1].span.start .. ".." .. s[1].span["end"])
            assert(s[2].span.start == 9 and s[2].span["end"] == 23, s[2].span.start .. ".." .. s[2].span["end"])
            "#,
        );
    }
}
