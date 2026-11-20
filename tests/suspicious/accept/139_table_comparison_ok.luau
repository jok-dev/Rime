--!rime
-- Near-misses of R0607 that must stay silent: comparisons where no side
-- is a table constructor.
local EMPTY: { number } = table.freeze({})

local function hasScores(scores: { number }): boolean
	return #scores > 0
end

local function isEmpty(scores: { number }): boolean
	return next(scores) == nil
end

local function isTheSharedEmpty(scores: { number }): boolean
	return scores == EMPTY -- identity against a table that already exists
end

local function longer(a: { number }, b: { number }): boolean
	return #a > #b
end

return table.freeze({
	hasScores = hasScores,
	isEmpty = isEmpty,
	isTheSharedEmpty = isTheSharedEmpty,
	longer = longer,
})
