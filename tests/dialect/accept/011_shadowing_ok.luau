--!rime
-- The shapes the shadowing rule must NOT flag:
--   - sibling scopes reusing a name (no enclosing binding — not shadowing)
--   - `_`-prefixed rebinds (deliberate discards)
--   - caching a global as a same-named local (no outer LOCAL involved)
local pairs = pairs

local function first(): number
	local value = 1
	return value
end

local function second(): number
	local value = 2 -- sibling scope, not a shadow
	return value
end

local function discards(t: { [string]: boolean }): number
	local _count = 0
	for _, _v in pairs(t) do
		local _count = 1 -- `_`-prefixed: exempt
		_count = _count + 0
	end
	return first() + second() + _count
end

return table.freeze({ discards = discards })
