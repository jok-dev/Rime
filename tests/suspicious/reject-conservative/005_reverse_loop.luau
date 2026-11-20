--!rime
-- Suspicious reverse loop (R0605, selene parity 2026-08-21).
--
-- WHY REJECTED: `for i = #t, 1` counts UP (default step +1), so the body
-- never runs when the table holds more than one element — the shape reads
-- as a reverse walk that forgot its `-1`.
-- WHAT WOULD FIX IT: `for i = #t, 1, -1`.
local function drain(items: { number }): number
	local total = 0
	--~ ERROR[R0605]: `for` from `#items` down to 1 without a `-1` step
	for i = #items, 1 do
		total += items[i]
	end
	return total
end

return table.freeze({ drain = drain })
