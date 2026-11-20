--!rime
-- `getB` is a LOCAL function nested inside `getA`'s own body, calling back
-- into `getA` as an upvalue — genuine mutual recursion with no double
-- binding of either name (the classic `local getA, getB; getA = function()
-- ... end` pre-declare idiom double-binds `getA`, which `frame_of`'s "no
-- SSA" poisoning would swallow before the evaluator ever saw the
-- recursion — and a plain sibling `local function getB()` declared AFTER
-- `getA` is not even valid Luau to call FROM `getA` — it is an undefined
-- global there until its own `local` runs).
--
-- The evaluator's cycle guard (`Cx::visited`) breaks the loop by degrading
-- the SECOND visit of either body to Unknown rather than looping forever —
-- the whole return-value candidate set collapses to a
-- plain R0502, no extra note (never a wrong path, but no explanation to
-- offer either).
local M = {}
local function getA()
	local function getB()
		return getA()
	end
	return getB()
end
M.Sub = getA()
return M
