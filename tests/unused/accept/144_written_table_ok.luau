--!rime
-- Near-misses of the write-only case of R0013 that must stay silent: a
-- table that is written to and then seen by someone.
local function runningTotals(scores: { number }): { number }
	local totals = {}
	local sum = 0
	for index, score in scores do
		sum += score
		totals[index] = sum
	end
	return totals -- returned
end

local function countDistinct(values: { string }): number
	local seen: { [string]: boolean } = {}
	local distinct = 0
	for _, value in values do
		if not seen[value] then -- a field is read
			seen[value] = true
			distinct += 1
		end
	end
	return distinct
end

local function append(values: { number }): number
	local copy = {}
	for _, value in values do
		copy[#copy + 1] = value -- the length is read
	end
	return #copy
end

return table.freeze({
	last = runningTotals({ 1, 2, 3 })[3],
	distinct = countDistinct({ "a", "b", "a" }),
	appended = append({ 4, 5 }),
})
