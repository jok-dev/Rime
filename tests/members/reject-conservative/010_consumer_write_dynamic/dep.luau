--!rime
-- Closed module; the dynamic write in main.luau is the case.
local dep = {}

function dep.get(key: string)
	print(key)
end

return table.freeze(dep)
