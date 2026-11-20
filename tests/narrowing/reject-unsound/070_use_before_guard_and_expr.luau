--!rime
-- R0202 where the check is the `and` short-circuit, not an `if`: `item and
-- item.suffix` tests `item` for nil — one line after `item.name` already
-- indexed it. A nil `item` never reaches the expression.
local function label(item): string
	local name = item.name
	local suffix = item and item.suffix or ""
	--~ ERROR[R0202]: `item` is checked for nil after it was already indexed
	return name .. suffix
end

return table.freeze({ label = label })
