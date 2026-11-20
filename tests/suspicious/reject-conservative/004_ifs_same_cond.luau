--!rime
-- Repeated elseif condition (R0604, selene parity 2026-08-21).
--
-- WHY REJECTED: the first arm already claims every case where the condition
-- is true, so the repeating arm can never run.
-- WHAT WOULD FIX IT: the condition the second arm was meant to check.
local function classify(n: number): string
	if n > 10 then
		return "big"
	--~ ERROR[R0604]: this `elseif` repeats an earlier condition
	elseif n > 10 then
		return "bigger"
	end
	return "small"
end

return table.freeze({ classify = classify })
