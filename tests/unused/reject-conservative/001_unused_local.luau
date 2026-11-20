--!rime
-- Unused local (R0013, decided 2026-08-21).
--
-- WHY REJECTED: valid Luau — `scratch` simply sits there — but a binding
-- nobody reads is either leftover clutter or a typo'd name, and Rime cannot
-- tell which.
-- WHAT WOULD FIX IT: delete it, or prefix with `_` if the binding is
-- deliberate. Downgrade with `unused-local = "off"` in rime.toml.
local function compute(items: { number }): number
	local total = 0
	local scratch = {}
	--~ WARNING[R0013]: `scratch` is never read
	for _, n in ipairs(items) do
		total += n
	end
	return total
end

return table.freeze({ compute = compute })
