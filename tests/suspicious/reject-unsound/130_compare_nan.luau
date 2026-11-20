--!rime
-- Comparison with NaN (R0610, selene parity `compare_nan`, 2026-11-20).
--
-- WHY REJECTED: NaN is not equal to anything, itself included, so `n ==
-- 0 / 0` is always false and `n ~= 0 / 0` is always true, whatever `n`
-- holds. Nothing at runtime can change that (`__eq` is consulted only for
-- two tables), which is why this report is firm and not conservative.
-- WHAT WOULD FIX IT: `n ~= n`, which is true only for NaN.
local function isNaN(n: number): boolean
	--~ ERROR[R0610]: comparison with NaN is always false
	return n == 0 / 0
end

local function isNumber(reading: { value: number }): boolean
	--~ ERROR[R0610]: comparison with NaN is always true
	return reading.value ~= 0 / 0
end

return table.freeze({ isNaN = isNaN, isNumber = isNumber })
