--!rime
-- Function attributes Luau knows (R0044 stays silent): `@native` and
-- `@checked`, alone or together, on a declaration, a local function and a
-- function expression. Selene's `unknown_function_attribute` knows only
-- `@native` and reports `@checked`; Luau compiles it, so Rime does not
-- follow Selene there. (`@deprecated` is corpus 151.)
local M = {}

@native
function M.fast(a: number, b: number): number
	return a + b
end

@checked @native
local function scale(n: number): number
	return n * 2
end

M.callback = @native function(n: number): number
	return scale(n) + 1
end

return table.freeze({ fast = M.fast(1, 2), callback = M.callback(4) })