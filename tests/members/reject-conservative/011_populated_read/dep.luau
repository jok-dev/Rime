--!rime
-- Closed module; main.luau injects a member then reads it back.
local dep = {}

function dep.get(key: string)
	print(key)
end

return table.freeze(dep)
