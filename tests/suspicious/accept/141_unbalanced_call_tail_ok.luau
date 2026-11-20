--!rime
-- Near-misses of R0608 that must stay silent: a value list that is short
-- but ends in a call or `...`, which may give several values.
local function pair(): (number, number)
	return 2, 3
end

local function spread(...: number): number
	local first, second, third = 1, pair() -- 1, 2, 3
	local head, tail = ... -- as many as were passed
	local one, two = pair()
	return first + second + third + (head or 0) + (tail or 0) + one + two
end

return table.freeze({ total = spread(10, 20) })
