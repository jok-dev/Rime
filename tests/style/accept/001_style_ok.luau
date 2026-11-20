--!rime
-- Near-misses of the style family (R0701–R0704) that must stay silent.
local function waitLoops(): number
	local n = 0
	while os.clock() < 0 do -- header carries the effect: the wait-loop idiom
	end
	repeat
	until os.clock() >= 0 -- same, repeat form
	n += 1
	return n
end

local function filteredCopy(source: { [string]: number }): { [string]: number }
	local copy = {}
	for key, value in pairs(source) do
		if value > 0 then -- filtering: not a clone, keep the loop
			copy[key] = value
		end
	end
	return copy
end

local function mergeInto(target: { [string]: number }, source: { [string]: number }): { [string]: number }
	for key, value in pairs(source) do
		target[key] = value -- destination is not fresh: a merge, not a clone
	end
	return target
end

local function plainTables(): number
	local array = { 1, 2, 3 }
	local map = { kind = "list", size = 3 }
	return #array + map.size
end

return table.freeze({
	waitLoops = waitLoops,
	filteredCopy = filteredCopy,
	mergeInto = mergeInto,
	plainTables = plainTables,
})
