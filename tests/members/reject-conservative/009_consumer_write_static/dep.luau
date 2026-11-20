--!rime
-- Closed module; the write in main.luau is the case.
local dep = {}

function dep.grant(player: string)
	print(player)
end

return table.freeze(dep)
