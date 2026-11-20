--!rime
-- NOT R0202: the only index of `item` before the guard sits behind
-- `item and ...`, which proves `item` non-nil for the rhs. A guarded index
-- is not evidence that the later check is late — nothing indexes a nil
-- `item` on any path.
local function label(item): string
	local name = item and item.name or "?"
	if not item then
		return "none: " .. name
	end
	return name
end

return table.freeze({ label = label({ name = "x" }) })
