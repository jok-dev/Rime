--!rime
-- Calls R0304 leaves alone: the name can hold something this file does not
-- define, one of its functions is variadic, or the call fits the widest
-- definition.
local function pick(narrow: boolean): (...number) -> number
	local add = function(a: number): number
		return a
	end
	if not narrow then
		add = function(a: number, b: number): number
			return a + (b or 0)
		end
	end
	-- Fits the two-parameter definition.
	return add
end

local function total(...: number): number
	local sum = 0
	for _, n in { ... } do
		sum += n
	end
	return sum
end

local function apply(callback: (number, number) -> number): number
	-- A parameter: the caller decides what it is.
	return callback(1, 2)
end

local combine = pick(false)
-- Bound to a call's result, not to a function written here.
local first = combine(1, 2)
-- Variadic: any count is taken.
local second = total(1, 2, 3)

return table.freeze({ first = first, second = second, third = apply(function(a: number, b: number): number
	return a + b
end) })