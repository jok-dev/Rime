--!rime
-- The same alias name declared twice with DIFFERENT initializers stays
-- poisoned: the env cannot know which binding a require site sees
-- (scope is deliberately not modelled), so ambiguity degrades to
-- R0502 Dynamic — never to a possibly-wrong path. BOTH sites pay,
-- including the one whose own binding would have resolved: that is the
-- price of the no-scope design, and the identical-initializer
-- exception (058) is its only relaxation.
local function a(): number
	local P = script.Parent
	local Economy = require(P.dep)
	--~ ERROR[R0502]: require target is not statically resolvable
	return Economy.balance("sam")
end

local function b(): number
	local P = script
	local Economy = require(P.dep)
	--~ ERROR[R0502]: require target is not statically resolvable
	return Economy.balance("sam")
end

return table.freeze({ a = a, b = b })
