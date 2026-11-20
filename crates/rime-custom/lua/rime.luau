--[[
The `rime` stdlib for custom project rules (rime-rules/*.luau).

Every node the bridge hands you is a plain table:
    { kind = "<Kind>", span = { start = <byte>, ["end"] = <byte> }, parent = <node>, ... }

Kinds and their fields (HIR names, see crates/rime-hir/src/lib.rs):
    Body            params (Param list), has_varargs, return_ty?, end_span?, block   -- the file root
    Function        same fields as Body (incl. end_span?, the `end` keyword)         -- any closure / declared body
    Block           stmts (list), terminator? (Return | Break | Continue)
    Return          exprs (list)
    Break, Continue
    Local           decls (Decl list), init (expr list)                        -- `local` and `const`
    Decl            name, ty? { rendered, admits_nil, span }, is_const         -- is_const: declared with `const`
    Param           name, ty?
    Assign          targets (Name | Index | expr list), values (expr list)
    CompoundAssign  target, value
    Call            base, args (list), method?                                 -- statement or expression
    Do              body (Block)
    If              cond, then (Block), else_ifs (ElseIf list), or_else? (Block)
    ElseIf          cond, block
    While           cond, body
    Repeat          body, until_
    NumericFor      var (Decl), start, end_, step?, body
    GenericFor      vars (Decl list), exprs (list), body
    LocalFunction   decl (Decl), body (Function)                               -- `local function` and `const function`
    FunctionDecl    root, path (string list), is_method, assigns_root, body (Function)
    Name            name
    Index           base, key? | name?, name_span?
    BinOp           op ("Add" .. "Or"), lhs, rhs
    Not             operand
    UnOp            op ("Neg" | "Len"), operand
    Table           fields (Field list)
    Field           name? + name_span?, key?, value
    IfExpr          arms (Arm list)
    Arm             cond?, value
    Literal         prim ("Number" | "String" | "Boolean" | "Nil"), text?
    Composite       parts (list)
    Leaf

Spans are byte offsets into file.src. A node with no span of its own
carries the union of its children's (a BinOp, an Index) or its parent's
(a Leaf) — approximate, but always present.

`ctx:fail` calls made before a rule throws are discarded — the R0025 for
the throw is the only output of that run.

rime.toml can turn a rule off, lower an `error` rule to `warn`, or raise
a `severity = "warn"` rule to `error` so it fails the run.
]]

local rime = {}

local function isNode(v)
	return type(v) == "table" and type(v.kind) == "string"
end

-- Child nodes in source order (by span start, then field name).
local function children(node)
	local out = {}
	for k, v in pairs(node) do
		if k ~= "parent" and k ~= "span" then
			if isNode(v) then
				table.insert(out, { key = tostring(k), node = v })
			elseif type(v) == "table" then
				for i, item in ipairs(v) do
					if isNode(item) then
						table.insert(out, { key = tostring(k) .. "." .. i, node = item })
					end
				end
			end
		end
	end
	table.sort(out, function(a, b)
		local sa, sb = a.node.span.start, b.node.span.start
		if sa ~= sb then
			return sa < sb
		end
		return a.key < b.key
	end)
	local nodes = {}
	for _, c in ipairs(out) do
		table.insert(nodes, c.node)
	end
	return nodes
end
rime.children = children

--- Preorder walk. `fn(node)` returning "skip" prunes that subtree.
function rime.walk(node, fn)
	if fn(node) == "skip" then
		return
	end
	for _, child in ipairs(children(node)) do
		rime.walk(child, fn)
	end
end

--- Every node of the given kind, preorder.
function rime.find(root, kind)
	local out = {}
	rime.walk(root, function(n)
		if n.kind == kind then
			table.insert(out, n)
		end
	end)
	return out
end

--- Every call, statement or expression.
function rime.calls(root)
	return rime.find(root, "Call")
end

--- "a.b.c" for a dotted base, "a:m" for a method call, nil when the base
--- is not a name chain (a call result, a bracket index...).
function rime.callee_name(call)
	local parts = {}
	local e = call.base
	while e do
		if e.kind == "Name" then
			table.insert(parts, 1, e.name)
			break
		elseif e.kind == "Index" and e.name ~= nil then
			table.insert(parts, 1, e.name)
			e = e.base
		else
			return nil
		end
	end
	if #parts == 0 then
		return nil
	end
	local name = table.concat(parts, ".")
	if call.method then
		return name .. ":" .. call.method
	end
	return name
end

local function collect(stmt, out)
	if stmt.kind == "Assign" then
		for i, target in ipairs(stmt.targets) do
			table.insert(out, { target = target, value = stmt.values[i], stmt = stmt })
		end
	elseif stmt.kind == "Local" then
		for i, decl in ipairs(stmt.decls) do
			table.insert(out, { target = decl, value = stmt.init[i], stmt = stmt })
		end
	end
end

--- { target, value?, stmt } for every Assign target and Local decl, any depth.
function rime.assignments(root)
	local out = {}
	rime.walk(root, function(n)
		collect(n, out)
	end)
	return out
end

--- Same, restricted to the root block's own statements.
function rime.root_assignments(root)
	local out = {}
	for _, stmt in ipairs(root.block.stmts) do
		collect(stmt, out)
	end
	return out
end

--- The root block's `return` expressions, or nil.
function rime.module_return(root)
	local term = root.block.terminator
	if term and term.kind == "Return" then
		return term.exprs
	end
	return nil
end

return rime
