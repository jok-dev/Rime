--!rime
-- A correctly-wired lazy-require stub — matching live assignment, path
-- resolves — guarded with `nil` instead of `false`. `nil and require(...)`
-- never runs either, so it is the same idiom, and which falsy literal a
-- project writes is its own business: nothing is broken, nothing is
-- reported.
local Economy = nil and require(script.Parent.dep)

local function init()
	Economy = require(script.Parent.dep)
end

local function balance()
	return Economy
end

return table.freeze({ init = init, balance = balance })
