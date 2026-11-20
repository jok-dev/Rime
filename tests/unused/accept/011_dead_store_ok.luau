--!rime
-- Stores that look dead to a careless liveness pass but are not:
--   - a default read on the path that skips the override;
--   - a loop-carried value read on the back edge;
--   - a store a nested closure reads before the overwrite (capture makes
--     liveness unknowable from the outer body alone — capture machinery).
local function fallback(flag: boolean): string
	local result = "default"
	if flag then
		result = "special"
	end
	return result
end

local function counterSum(n: number): number
	local total = 0
	for _i = 1, n do
		total += 1
	end
	return total
end

local function tracker(start: number): number
	local hits = start
	local function bump(by: number): number
		hits += by
		return hits
	end
	bump(1)
	hits = 10
	return hits
end

return table.freeze({
	fallback = fallback,
	counterSum = counterSum,
	tracker = tracker,
	check = tracker(0),
})
