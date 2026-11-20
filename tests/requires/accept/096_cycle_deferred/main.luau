--!rime
--!no-exec
-- The lazy-require idiom: the stub at the top never executes, and the real
-- require runs inside `init`, after both modules have loaded. dep requires
-- main at load time, but the edge from main to dep is deferred: no ring.
local Dep = false and require(script.Parent.dep)

local function init()
	Dep = require(script.Parent.dep)
end

local function value()
	return Dep.value()
end

return table.freeze({ init = init, value = value })
