--!rime
-- A broken `false and require(...)` STUB whose variable is also assigned
-- the SAME require path by live code later in the file. The stub is the
-- lazy-require idiom (cyclic deps: document the dependency at the top,
-- load it for real in init). One broken path must yield ONE error — the
-- live require carries it; the stub stays silent.
--
-- WHY CONSERVATIVE: same caveat as 046 — a loader can clone instances in
-- at runtime; Rime cannot see that, says so, and fails by default.
local Ghost = false and require(script.Parent.Ghost)

local function init()
	Ghost = require(script.Parent.Ghost)
	--~ ERROR[R0501]: cannot resolve this require
end

local function get()
	return Ghost
end

return table.freeze({ init = init, get = get })
