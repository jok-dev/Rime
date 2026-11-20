--!rime
-- The `nil and` guard on a BROKEN, never-assigned stub. The stub matrix
-- must recognize the nil guard so the R0501 note stays honest — "guarded
-- by `nil and`, never runs", NOT the throws-at-load claim a plain broken
-- require gets. The guard itself is not reported: `nil` and `false` are
-- both the stub idiom.
local Ghost = nil and require(script.Parent.Ghost)
--~ ERROR[R0501]: cannot resolve this require

local function get()
	return Ghost
end

return table.freeze({ get = get })
