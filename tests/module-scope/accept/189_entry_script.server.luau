--!rime
--~ OPTION[R0312]: on
-- An entry script, by its name (`*.server.luau`): it exists to do work as
-- it starts, and the rule leaves it alone.

local function start(name: string): string
	return "started " .. name
end

local log = {}
for _, name in { "first", "second" } do
	table.insert(log, start(name))
end
print(#log)
