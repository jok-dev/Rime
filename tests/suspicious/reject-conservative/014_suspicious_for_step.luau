--!rime
-- Numeric `for` whose STEP contradicts its range (R0609, added 2026-09-04
-- alongside the R0605 widening).
--
-- WHY REJECTED: a negative step with start below end never enters the
-- body; a step of 0 never advances — Luau either skips the body outright
-- (start < end) or repeats it forever (start >= end). Neither is ever
-- what was meant.
-- WHAT WOULD FIX IT: `for i = 1, 8` / `for i = 8, 1, -1`, and a non-zero step.
local function upward(): { number }
	local out = {}
	--~ ERROR[R0609]: `for` from 1 up to 8 with a negative step never runs
	for i = 1, 8, -1 do
		table.insert(out, i)
	end
	return out
end

local function stuck(items: { number }): number
	local total = 0
	--~ ERROR[R0609]: `for` with a step of 0 never advances
	for i = 1, #items, 0 do
		total += items[i]
	end
	return total
end

return table.freeze({ upward = upward, stuck = stuck })
