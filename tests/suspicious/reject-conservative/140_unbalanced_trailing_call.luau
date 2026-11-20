--!rime
-- Too many values, the last one a call (R0608, selene parity
-- `unbalanced_assignments`, 2026-11-20).
--
-- WHY REJECTED: `local total = base, bonus()` has one name and two
-- values. A trailing call can stretch a list that is too short, but it
-- cannot shorten one that is already too long: `bonus()` runs and every
-- result it returns is dropped. Usually a `+` or a second name went
-- missing in an edit.
-- WHAT WOULD FIX IT: a name for each value (`local total, extra = ...`),
-- or the operator that was meant (`base + bonus()`).
local function bonus(): number
	return 5
end

local function score(base: number): number
	--~ ERROR[R0608]: 1 name but 2 values
	local total = base, bonus()
	return total
end

return table.freeze({ score = score })
