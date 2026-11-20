--!rime
-- The if-EXPRESSION form of a condition binding, with `const`, on an
-- `elseif` arm. Same experimental syntax and the same answer as case 004:
-- reported once, at the keyword, and nothing else in the file is checked
-- (the undefined `missing` below draws no second diagnostic).
local cache = {}

local function lookup(key: string): number
	return if key == "" then 0 elseif const entry = cache[key] then entry else missing
	--~ ERROR[R0900]: `elseif const` bindings are experimental Luau syntax that Rime does not understand yet
end

return table.freeze({ lookup = lookup })
