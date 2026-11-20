--!rime
-- Dead store (R0104, decided 2026-08-21).
--
-- WHY REJECTED: valid Luau, but every path overwrites the assigned value
-- before anything reads it — the store is wasted work at best, a logic slip
-- (wrong variable, misplaced default) at worst, and Rime cannot tell which.
-- WHAT WOULD FIX IT: assign where the value is first needed, or drop the
-- store. Downgrade with `dead-store = "off"` in rime.toml.
local function straight(): number
	local x = 1
	--~ WARNING[R0104]: value assigned to `x` is never read
	x = 2
	return x
end

local function label(flag: boolean): string
	local result = "pending"
	--~ WARNING[R0104]: value assigned to `result` is never read
	if flag then
		result = "yes"
	else
		result = "no"
	end
	return result
end

return table.freeze({ straight = straight, label = label })
