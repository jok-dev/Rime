--!rime
--!no-exec
-- The lazy-require idiom used CORRECTLY: a `false and require(...)` stub
-- at the top documents a cyclic dependency, and live code later assigns
-- the SAME variable from the SAME path, which resolves. Zero diagnostics
-- — the idiom itself is not a lint target.
local Economy = false and require(script.Parent.dep)

local function init()
	Economy = require(script.Parent.dep)
end

local function balance()
	return Economy
end

return table.freeze({ init = init, balance = balance })
