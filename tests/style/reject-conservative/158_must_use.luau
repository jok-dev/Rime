--!rime
-- A library call that only computes a value, on a line of its own (R0708,
-- Selene's `must_use`).
--
-- WHY REJECTED: the call returns its answer and changes nothing else, so
-- with the answer dropped the statement does no work. The usual cause is
-- a lost `local x =` or `return`.
-- WHAT WOULD FIX IT: use the result, or remove the call.
local function label(coins: number, limit: number): string
	--~ WARNING[R0708]: the result of `math.min` is not used
	math.min(coins, limit)
	--~ WARNING[R0708]: the result of `string.format` is not used
	string.format("%d coins", coins)
	if coins > limit then
		--~ WARNING[R0708]: the result of `tostring` is not used
		tostring(limit)
	end
	return tostring(coins)
end

return table.freeze({ label = label })