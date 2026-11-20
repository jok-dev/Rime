--!rime
-- NOT R0202: `type(value) == "table"` discriminates tables from everything
-- else — it is a type test that happens to exclude nil, not a nil check.
-- Indexing `value` first does not make it dead (a string also has no
-- `.name`, and the index would have thrown only for nil). Only the pure
-- nil tests (`x`, `not x`, `x == nil`, `type(x) == "nil"`) are R0202 sites.
local function describe(value): string
	local name = value.name
	if type(value) == "table" then
		return "table " .. name
	end
	return "other"
end

return table.freeze({ describe = describe({ name = "x" }) })
