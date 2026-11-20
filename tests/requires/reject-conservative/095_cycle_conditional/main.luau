--!rime
-- The ring closes only through a require under a top-level `if`: main
-- loads dep on the server, dep loads main unconditionally. Whether the
-- ring is ever reachable depends on which context runs, so Rime says so
-- instead of claiming a throw.
--
-- WHY CONSERVATIVE: a `RunService:IsServer()` split may keep the two
-- edges from ever running together; Rime cannot tell.
local RunService = game:GetService("RunService")

local Dep = nil
if RunService:IsServer() then
	Dep = require(script.Parent.dep)
	--~ ERROR[R0509]: circular require
end

local function dep()
	return Dep
end

return table.freeze({ dep = dep })
