--!rime
-- Unused name in a multi-assignment (R0013, decided 2026-08-21).
--
-- WHY REJECTED: `a` binds the first return value and nobody reads it. The
-- position is load-bearing — `b` needs the second slot — so the fix is the
-- `_` prefix, not deletion.
-- WHAT WOULD FIX IT: rename to `_a`.
local function fetch(): (number, number)
	return 1, 2
end

local function pick(): number
	local a, b = fetch()
	--~ WARNING[R0013]: `a` is never read
	return b
end

return table.freeze({ pick = pick })
