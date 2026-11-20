--!rime
-- `assert(cond)` returns only when `cond` held, so everything after it is
-- guarded the same way the body of `if cond then` is.
local function increment(v: number?): number
	assert(v)
	return v + 1
end

local function incrementWithMessage(v: number?): number
	assert(v ~= nil, "v is required")
	return v + 1
end

local function incrementTyped(v: number?): number
	assert(type(v) == "number", "v must be a number")
	return v + 1
end

local function sum(a: number?, b: number?): number
	assert(a and b, "both are required")
	return a + b
end

-- The value form: `assert` hands back its argument.
local function double(v: number?): number
	local checked = assert(v, "v is required")
	return v + checked
end

return table.freeze({
	increment = increment(1),
	incrementWithMessage = incrementWithMessage(1),
	incrementTyped = incrementTyped(1),
	sum = sum(1, 2),
	double = double(2),
})
