-- rime: allow-file(undefined-variable, "rule files run inside Rime's rule VM, where `rime` is a global")
-- Every Cmdr command definition returns `{ Name = "<string>", ... }`.
-- Definitions may build the table in a local first (`local cmd = { ... }`
-- then `return cmd`); the sibling `*Server.lua` implementation files return
-- a function and are skipped. Demonstrates `module_return` plus resolving a
-- returned name back to its root-level declaration.
return {
	files = "src/**/Commands/**/*.lua",
	check = function(file, ctx)
		if file.path:match("Server%.lua$") then
			return
		end
		local ret = rime.module_return(file.root)
		local value = ret and ret[1]
		if value and value.kind == "Name" then
			-- `return command`: the last root-level assignment to that name.
			for _, a in rime.root_assignments(file.root) do
				local t = a.target
				if (t.kind == "Decl" or t.kind == "Name") and t.name == value.name and a.value then
					value = a.value
				end
			end
		end
		if value == nil or value.kind ~= "Table" then
			ctx:fail(value or { start = 0, ["end"] = 0 }, "command definition must return a table")
			return
		end
		for _, field in ipairs(value.fields) do
			if field.name == "Name" and field.value.kind == "Literal" and field.value.prim == "String" then
				return
			end
		end
		ctx:fail(value, "command definition has no string `Name` field")
	end,
}
