--!rime
-- Divide by zero (R0602, selene parity 2026-08-21).
--
-- WHY REJECTED: Luau raises no error on `x / 0` — the result is inf or nan,
-- which then spreads silently through every downstream computation.
-- WHAT WOULD FIX IT: a non-zero divisor, or a named constant (`math.huge`)
-- if the infinity is deliberate.
local function ratio(n: number): number
	--~ WARNING[R0602]: division by zero
	return n / 0
end

local function wrap(n: number): number
	--~ WARNING[R0602]: modulo by zero
	return n % 0
end

return table.freeze({ ratio = ratio, wrap = wrap })
