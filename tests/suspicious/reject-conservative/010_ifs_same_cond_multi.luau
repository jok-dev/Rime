--!rime
-- The SAME condition guards two later arms: each duplicate arm gets its
-- own diagnostic, anchored at that arm and pointing back at the first
-- check (one per occurrence; a duplicate arm is one occurrence, so its
-- diagnostic cites only its FIRST earlier match, not every earlier pair).
-- Regression case for the `return` that used to stop after the first pair.
local function pick(n: number): number
	if n > 10 then
		return 3
	elseif n > 10 then
		return 2
	elseif n > 10 then
		return 1
	else
		return 0
	end
end

return pick
--~ ERROR[R0604]: this `elseif` repeats an earlier condition
