--!rime
-- Two modules require each other at load time. Roblox throws
-- `Requested module was required recursively` the moment the second one
-- loads — a hard runtime error, so this is firm, not conservative.
local Dep = require(script.Parent.dep)
--~ ERROR[R0509]: circular require

local function hello()
	return Dep.x
end

return table.freeze({ hello = hello })
