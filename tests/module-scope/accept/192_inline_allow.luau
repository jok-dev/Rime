--!rime
--~ OPTION[R0312]: on
-- One site is let through with the usual allow comment, on the line or
-- on the line above; on a loop it covers the loop.

local Registry = {}

local function build(...: string): { string }
	return { ... }
end

local names = build("first", "second") -- rime: allow(module-scope-computation, "two names, built once")

-- rime: allow(module-scope-computation, "builds the lookup once; two entries")
for index, name in names do
	Registry[name] = index
end

-- rime: allow(module-scope-computation, "a function called where it is written: the comment goes on its statement")
local doubled = (function()
	return #names * 2
end)()
Registry.Doubled = doubled

-- rime: allow(R0312, "the code works as well as the name")
local started = os.clock()
Registry.Started = started

return Registry
