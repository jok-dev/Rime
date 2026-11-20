--!rime
--~ OPTION[R0312]: on
-- A few built-in functions call the function they are handed before they
-- return: `table.sort`, `table.foreach`, `table.foreachi`, `string.gsub`,
-- `pcall` and `xpcall`. That function's body is module scope too. Any
-- other call only keeps the function, and so does a table.

local Ranks = { first = 1, second = 2 }
local names = { "second", "first" }

local function rank(name: string): number
	return Ranks[name]
end

table.sort(names, function(left, right)
	return rank(left) < rank(right)
	--~ WARNING[R0312]: `rank` is called at module scope
	--~ WARNING[R0312]: `rank` is called at module scope
end)

local shouted = string.gsub("first second", "%a+", function(word)
	return tostring(rank(word))
	--~ WARNING[R0312]: `rank` is called at module scope
end)

local ok = pcall(function()
	rank("first")
	--~ WARNING[R0312]: `rank` is called at module scope
end)

local Lookup = setmetatable({}, {
	__index = function(_, name)
		return rank(name)
	end,
})

local later = {}
table.insert(later, function()
	return rank("second")
end)

return { names = names, shouted = shouted, ok = ok, Lookup = Lookup, later = later }
