--!rime
-- Reverse-looking loops that R0605 used to miss (widened 2026-09-04):
-- an explicit POSITIVE step, and an end of 0.
--
-- WHY REJECTED: `for i = #t, 1, 1` spells the step out and still counts
-- up — the body runs at most once (only when `#t <= 1`). `for i = #t, 0`
-- is the same shape one further down (and 0 is not an array index).
-- WHAT WOULD FIX IT: a `-1` step: `for i = #t, 1, -1`.
local function drainExplicit(items: { number }): number
	local total = 0
	--~ ERROR[R0605]: `for` from `#items` down to 1 without a `-1` step
	for i = #items, 1, 1 do
		total += items[i]
	end
	return total
end

local function drainToZero(items: { number }): number
	local total = 0
	--~ ERROR[R0605]: `for` from `#items` down to 0 without a `-1` step
	for i = #items, 0 do
		total += items[i] or 0
	end
	return total
end

return table.freeze({ drainExplicit = drainExplicit, drainToZero = drainToZero })
