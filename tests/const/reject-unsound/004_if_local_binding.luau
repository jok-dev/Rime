--!rime
-- `if local name = expr then` (and `if const`, `elseif local`, the same in
-- if-expressions) is an EXPERIMENTAL Luau syntax: a proposal still open
-- upstream, compiled only behind a flag. Rime's parser reads it, but Rime
-- has no model yet for a name that is declared inside a condition.
--
-- Checking the file while ignoring the binding would report every use of
-- `entry` as an undefined variable, so Rime says what it cannot do and
-- checks nothing in the file — the same outcome as before the parser
-- could read this syntax, with an honest reason.
local cache = {}

local function lookup(key: string): number
	if local entry = cache[key] then
	--~ ERROR[R0900]: `if local` bindings are experimental Luau syntax that Rime does not understand yet
		return entry
	end
	return 0
end

return table.freeze({ lookup = lookup })
