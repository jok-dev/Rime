--!rime
-- A broken `false and require(...)` stub that is NEVER live-required:
-- no assignment of the variable anywhere in the file. Either the feature
-- is dead or the path rotted with nobody loading it. Still an error —
-- but an honest one: the guard means nothing throws at runtime, so the
-- diagnostic must not claim it does.
--
-- WHY CONSERVATIVE: same caveat as 046 — a loader can clone instances in
-- at runtime; Rime cannot see that.
local Ghost = false and require(script.Parent.Ghost)
--~ ERROR[R0501]: cannot resolve this require

local function get()
	return Ghost
end

return table.freeze({ get = get })
