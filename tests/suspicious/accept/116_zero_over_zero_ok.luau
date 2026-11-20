--!rime
-- `0 / 0` is how Luau code writes NaN: there is no `math.nan`. R0602 stays
-- silent on a literal zero over a literal zero, as Selene's
-- `divide_by_zero` does, and `math.huge` is the way to write infinity.
local NAN = 0 / 0
local INF = math.huge

local function isNaN(n: number): boolean
	return n ~= n
end

local function sentinel(): number
	return -(0 / 0)
end

return table.freeze({
	nan = isNaN(NAN),
	sentinel = isNaN(sentinel()),
	inf = INF > 1,
})
