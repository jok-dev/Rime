--!rime
-- Mixed table constructor (R0703, selene parity 2026-08-21).
--
-- WHY REJECTED: one table serving as both array and map trips length
-- (`#t` ignores the keyed part), iteration order, and JSON encoding.
-- WHAT WOULD FIX IT: split into an array field plus named fields.
local function build(): { n: number }
	--~ WARNING[R0703]: array items mixed with keyed fields
	local t = { 1, 2, kind = "list", 3 }
	return { n = #t }
end

return table.freeze({ build = build })
