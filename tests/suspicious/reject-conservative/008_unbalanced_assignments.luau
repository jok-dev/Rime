--!rime
-- Unbalanced assignments (R0608, selene parity 2026-08-21).
--
-- WHY REJECTED: the name and value counts differ and every value is
-- single-width, so the extra names are nil (or the extra values are
-- discarded) — usually an edit slip. A short list ending in a multi-value
-- tail (a call, `...`) keeps the rule silent.
-- WHAT WOULD FIX IT: matching counts, or explicit nils if intended.
local function split(pair: { number }): (number, number)
	--~ ERROR[R0608]: 2 names but 1 value
	local first, second = pair[1]
	return first, second
end

local function firstOnly(a: number, b: number): number
	--~ ERROR[R0608]: 1 name but 2 values
	local keep = a, b
	return keep
end

return table.freeze({ split = split, firstOnly = firstOnly })
