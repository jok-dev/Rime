--!rime
-- With no settings the rule does not run at all.

local started = os.clock()
local names = {}
for index = 1, 3 do
	names[index] = tostring(index)
end

return { started = started, names = names }
