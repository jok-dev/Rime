--!rime
-- A table that is only written to (R0013, selene parity `unused_variable`
-- write-only case, 2026-11-20).
--
-- WHY REJECTED: `totals` is built by a constructor and every mention of
-- it stores a field. Nothing reads a field, returns the table or passes
-- it on, so nobody can see what was stored: the table and the work that
-- fills it are dead. Usually the `return totals` went missing.
-- WHAT WOULD FIX IT: return or use the table, or delete it and its writes.
local function tally(scores: { number }): number
	local totals = {}
	--~ WARNING[R0013]: `totals` is written to, but never read
	local sum = 0
	for index, score in scores do
		sum += score
		totals[index] = sum
	end
	totals.final = sum
	return sum
end

return table.freeze({ tally = tally })
