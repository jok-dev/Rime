--!rime
-- Near-misses of R0012 on number keys that must stay silent: distinct
-- number slots, a string key that looks like a number, and number keys
-- that only repeat one constructor level down.
local sparse = {
	[0] = "zero",
	[1] = "one",
	[-1] = "minus one",
	[1.5] = "one and a half",
	["1"] = "the string",
	nested = { [1] = "inner one" },
}

local list = { "a", "b", "c" }

return table.freeze({ sparse = sparse, list = list })
