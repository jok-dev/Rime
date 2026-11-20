--!rime
-- NOT R0202: in `flag and item or fallback` (and in a bare `flag and
-- item`), `item` is the VALUE the expression selects, not something it
-- tests — only the left operand of an `and`/`or` is a guard. Reading
-- `item.name` first therefore contradicts nothing here.
local function label(item, flag: boolean, fallback): string
	local name = item.name
	local chosen = flag and item or fallback
	local maybe = flag and item
	return name .. chosen.name .. (maybe and maybe.name or "")
end

return table.freeze({ label = label({ name = "x" }, true, { name = "y" }) })
