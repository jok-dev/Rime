--!rime
--~ OPTION[R0312]: on
-- A file that returns no value cannot be required, so it is an entry
-- script whatever its name is.

local function start(name: string): string
	return "started " .. name
end

local first = start("first")
print(first)
