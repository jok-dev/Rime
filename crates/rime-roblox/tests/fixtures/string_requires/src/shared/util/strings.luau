local strings = {}

function strings.grant(player: string, amount: number, reason: string): boolean
	print(player, amount, reason)
	return true
end

return table.freeze(strings)
