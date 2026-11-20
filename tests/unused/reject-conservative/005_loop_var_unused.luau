--!rime
-- Unused loop variables (R0013, decided 2026-08-21).
--
-- WHY REJECTED: loop variables are judged individually — an unread value
-- variable in a `pairs` loop, or an unread numeric-for counter, is the same
-- clutter-or-typo signal as any other unused local.
-- WHAT WOULD FIX IT: prefix with `_` (`for k, _v in pairs(map)`).
local function keys(map: { [string]: number }): { string }
	local out = {}
	for k, v in pairs(map) do
		--~ WARNING[R0013]: `v` is never read
		table.insert(out, k)
	end
	for i = 1, 3 do
		--~ WARNING[R0013]: `i` is never read
		table.insert(out, "pad")
	end
	return out
end

return table.freeze({ keys = keys })
