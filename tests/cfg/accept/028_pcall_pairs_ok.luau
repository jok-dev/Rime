--!rime
-- Legal shapes around R0110 (pcall discarded) and R0111 (mutation during
-- pairs):
--   - the pcall result is captured and checked, not discarded
--   - the write inside `pairs` targets a DIFFERENT table
--   - a write through the loop's own key variable updates a field the
--     traversal is visiting, which Lua defines
--   - the iterated table is mutated only AFTER the loop ends
local function tally(set: { [string]: boolean }): number
	local seen = {}
	local n = 0
	for key in pairs(set) do
		seen[key] = true -- different table: fine
		n += 1
	end
	set.checked = next(seen) ~= nil or n == 0 -- after the loop: fine
	return n
end

local function double(list: { number }): { number }
	for i, v in ipairs(list) do
		list[i] = v * 2 -- the current key names an existing field: defined
	end
	return list
end

local ok, err = pcall(function()
	error("boom")
end)
assert(ok == false and err ~= nil)

local counted = tally({ a = true, b = true })
assert(counted == 2)

local doubled = double({ 1, 2, 3 })
assert(doubled[3] == 6)

return table.freeze({ tally = tally, double = double })
