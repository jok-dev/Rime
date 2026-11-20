--!rime
--!no-exec
-- The SAME alias declared in two sibling function scopes with IDENTICAL
-- initializers must still resolve. A boot script that binds
-- one folder alias (`local P = script.Parent`) at module level AND
-- inside a function met the env's bound-more-than-once poison rule: the
-- name was dropped and every require through it went R0502. Identical initializers cannot
-- resolve to different places (any name they mention is itself either
-- consistent or poisoned), so keeping the alias is sound. Differing
-- initializers still poison — see 059.
local function a(): number
	local P = script.Parent
	local Economy = require(P.dep)
	return Economy.balance("sam")
end

local function b(): number
	local P = script.Parent
	local Economy = require(P.dep)
	return Economy.balance("sam")
end

return table.freeze({ a = a, b = b })
