--!rime
-- Inline allow, trailing form: the shadow is deliberate and vouched for
-- with a reason, so the file is clean. The directive must carry a reason —
-- see reject cases 003/004 for what happens when it does not.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n -- rime: allow(shadowing, "deliberate rebind; exercises inline allow")
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
