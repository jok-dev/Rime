local piece = {}

function piece.grant(player: string, amount: number, reason: string): boolean
	print(player, amount, reason)
	return true
end

return table.freeze(piece)
