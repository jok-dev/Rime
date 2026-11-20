//! The `rime` global: helpers written in Luau over the bridge's node
//! model. Helpers grow here, not in Rust.

use mlua::{Lua, Result, Table};

pub const RIME_LUA: &str = include_str!("../lua/rime.luau");

/// Evaluate the stdlib and bind it as global `rime`. Call before
/// `Lua::sandbox(true)`; sandbox mode makes globals read-only.
pub fn install(lua: &Lua) -> Result<()> {
    let stdlib: Table = lua.load(RIME_LUA).set_name("@rime.luau").eval()?;
    lua.globals().set("rime", stdlib)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(src: &str, lua_asserts: &str) {
        let module = rime_syntax::parse(src).expect("fixture parses");
        let lua = Lua::new();
        install(&lua).expect("stdlib loads");
        let root = crate::bridge::module_tree(&lua, &module).expect("bridge");
        lua.globals().set("t", root).unwrap();
        lua.load(lua_asserts).set_name("@asserts").exec().unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn walk_is_preorder_in_source_order_and_skips() {
        check(
            "local a = f(1)\nlocal b = g(h(2))\n",
            r#"
            local kinds = {}
            rime.walk(t, function(n)
                table.insert(kinds, n.kind)
                if n.kind == "Local" and n.decls[1].name == "b" then return "skip" end
            end)
            assert(kinds[1] == "Body" and kinds[2] == "Block" and kinds[3] == "Local", table.concat(kinds, ","))
            -- the second Local was skipped: no Call named g/h after it
            assert(#rime.find(t, "Call") == 3)
            local walked_calls = 0
            for _, k in ipairs(kinds) do if k == "Call" then walked_calls += 1 end end
            assert(walked_calls == 1, "skip pruned the subtree: " .. walked_calls)
            "#,
        );
    }

    #[test]
    fn calls_and_callee_names() {
        check(
            "a.b.c(1)\nx:m()\nlocal y = (f())()\nprint(1)\n",
            r#"
            local calls = rime.calls(t)
            assert(#calls == 5, #calls)
            assert(rime.callee_name(calls[1]) == "a.b.c")
            assert(rime.callee_name(calls[2]) == "x:m")
            -- the outer call's base is itself a call: no name chain
            local named = {}
            for _, c in ipairs(calls) do named[#named + 1] = tostring(rime.callee_name(c)) end
            assert(table.find(named, "nil") ~= nil, table.concat(named, ","))
            assert(rime.callee_name(calls[#calls]) == "print")
            "#,
        );
    }

    #[test]
    fn assignments_root_assignments_and_module_return() {
        check(
            "local M = {}\nM.Tag = \"catalog\"\nlocal function f()\n  M.other = 1\nend\nreturn M\n",
            r#"
            local all = rime.assignments(t)
            assert(#all == 3, #all)
            local roots = rime.root_assignments(t)
            assert(#roots == 2, #roots)
            assert(roots[1].target.kind == "Decl" and roots[1].target.name == "M" and roots[1].value.kind == "Table")
            assert(roots[2].target.kind == "Index" and roots[2].target.name == "Tag" and roots[2].value.text == "catalog")
            assert(roots[2].stmt.kind == "Assign")
            local ret = rime.module_return(t)
            assert(ret ~= nil and ret[1].kind == "Name" and ret[1].name == "M")
            "#,
        );
        check("local x = 1\n", "assert(rime.module_return(t) == nil)");
    }
}
