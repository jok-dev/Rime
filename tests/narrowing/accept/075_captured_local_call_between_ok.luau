--!rime
-- NOT R0202: `item` is captured by a closure that can rebind it, and a
-- call stands between the index and the check. The call may have run the
-- closure, so the value being checked is not the value that was indexed —
-- same aliasing rule R0201 lives by.
local function label(item): string
	local name = item.name
	local function clear()
		item = nil
	end
	clear()
	if not item then
		return name .. " (cleared)"
	end
	return name
end

return table.freeze({ label = label({ name = "x" }) })
