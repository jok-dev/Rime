--!rime
-- Closed module; the interesting part is in main.luau.
local dep = {}

function dep.grant(player: string)
	print(player)
end

return table.freeze(dep)
