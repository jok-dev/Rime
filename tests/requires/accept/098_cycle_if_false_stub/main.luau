--!rime
--!no-exec
-- The `if false then X = require(P) end` type-import idiom: the block never
-- runs, so it is no more an edge than `false and require(P)` is. dep
-- requires main at load time; main never loads dep. No ring.
local Dep = nil
if false then
	Dep = require(script.Parent.dep)
end

local function value()
	return Dep
end

return table.freeze({ value = value })
