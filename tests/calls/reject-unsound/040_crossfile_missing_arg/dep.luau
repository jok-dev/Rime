--!rime
-- The required module: `grant` demands three arguments.
local dep = {}

function dep.grant(player: string, amount: number, reason: string): boolean
	print(player, amount, reason)
	return true
end

return table.freeze(dep)
