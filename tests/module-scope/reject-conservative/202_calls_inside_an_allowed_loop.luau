--!rime
--~ OPTION[R0312]: allow-loops = true
-- With loops allowed, the calls inside are checked one by one, the loop
-- header included. The built-in lists apply there as anywhere.

local Lookup = {}
local names = { "first", "second" }

local function entries(list: { string })
	return ipairs(list)
end

local function label(index: number): string
	return "#" .. index
end

for index, name in entries(names) do
--~ WARNING[R0312]: `entries` is called at module scope
	Lookup[name] = label(index)
	--~ WARNING[R0312]: `label` is called at module scope
	Lookup[string.upper(name)] = tostring(index)
end

return Lookup
