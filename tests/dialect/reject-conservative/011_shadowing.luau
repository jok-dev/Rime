--!rime
-- Shadowing rejected by default (decided 2026-08-21).
--
-- WHY REJECTED: valid Luau — the inner `count` simply wins — but later reads
-- silently pick the inner binding, and Rime cannot tell a deliberate rebind
-- from a mistake.
-- WHAT WOULD FIX IT: rename one of them, or prefix the finished one with `_`.
-- Downgrade with `shadowing = "warn"` in rime.toml if migrating.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n
		--~ ERROR[R0011]: `count` shadows an earlier declaration
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
