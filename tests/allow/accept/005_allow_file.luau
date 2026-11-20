--!rime
-- File-level allow: one directive at the top (only comments/blanks above)
-- suppresses every matching diagnostic in the file — both shadows here.
-- rime: allow-file(shadowing, "legacy module, rebinds everywhere")
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n
		print(count)
	end
	return count
end

local function tally2(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n
		print(count)
	end
	return count
end

return table.freeze({ tally = tally, tally2 = tally2 })
