--!rime
--!no-exec
-- `@deprecated` is the third attribute Luau knows (R0044 stays silent).
-- Selene's `unknown_function_attribute` reports it; current Luau compiles
-- it, so Rime does not follow Selene there.
--
-- Static-only: the vendored Luau (0.663) predates `@deprecated` and
-- refuses it as an invalid attribute. lune 0.10.5 compiles it.
local M = {}

@deprecated
function M.old(n: number): number
	return n + 1
end

@deprecated @native
local function older(n: number): number
	return n + 2
end

return table.freeze({ old = M.old, older = older })