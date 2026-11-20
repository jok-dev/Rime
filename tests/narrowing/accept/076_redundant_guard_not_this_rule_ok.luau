--!rime
-- NOT R0202: the guard clause runs FIRST, so the index below it is guarded
-- and the second `if not item` is merely redundant — a dead check, but not
-- a late one. Redundant-condition detection is a different rule; this one
-- only speaks when an unguarded use precedes the check.
local function label(item): string
	if not item then
		return "none"
	end
	local name = item.name
	if not item then
		return "unreachable"
	end
	return name
end

return table.freeze({ label = label({ name = "x" }) })
