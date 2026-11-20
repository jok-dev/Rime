--!rime
-- Near-misses of R0610 compare-nan that must stay silent. The first three
-- are the ways to work with NaN that the report recommends; the rest are
-- the shapes Selene's `compare_nan` fixtures leave alone.
local NAN = 0 / 0

local function isNaN(n: number): boolean
	return n ~= n
end

local function isNumber(n: number): boolean
	return n == n
end

local function orNaN(n: number, ok: boolean): number
	return if ok then n else NAN
end

local function spread(n: number): number
	-- NaN in arithmetic is not a comparison.
	return n + 0 / 0
end

local function isZero(n: number): boolean
	return n == 0
end

return table.freeze({
	isNaN = isNaN,
	isNumber = isNumber,
	orNaN = orNaN,
	spread = spread,
	isZero = isZero,
})
