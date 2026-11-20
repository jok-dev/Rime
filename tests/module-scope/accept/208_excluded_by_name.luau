--!rime
--~ OPTION[R0312]: exclude = 208_*.luau
-- `exclude` names files the rule leaves alone for the project's own
-- reasons: this one is a module like any other, and is not looked at.

local started = os.clock()
local names = {}
for index = 1, 3 do
	names[index] = index * 2
end

return { started = started, names = names }
