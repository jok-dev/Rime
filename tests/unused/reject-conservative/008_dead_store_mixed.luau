--!rime
-- Dead store where only SOME paths overwrite (R0104): one branch overwrites
-- before any read, the other returns without touching the local — so the
-- note must not claim "every path overwrites".
local function greeting(flag: boolean): string
	--~ WARNING[R0104]: value assigned to `s` is never read
	local s = "pending"
	if flag then
		s = "yes"
		return s
	end
	return "no"
end

return table.freeze({ greeting = greeting })
