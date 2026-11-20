--!rime
-- Three modules in a load-time ring: main -> b -> c -> main. Every site on
-- the ring reports in its own file; the witness note names the whole ring.
local B = require(script.Parent.b)
--~ ERROR[R0509]: circular require
--~ NOTE: cycle: main.luau → b.luau → c.luau → main.luau

local function value()
	return B.value()
end

return table.freeze({ value = value })
