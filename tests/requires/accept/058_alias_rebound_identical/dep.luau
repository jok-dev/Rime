--!rime
local dep = {}

function dep.grant(player: string, amount: number, reason: string?): boolean
	print(player, amount, reason)
	return true
end

function dep.balance(player: string): number
	print(player)
	return 0
end

return table.freeze(dep)
