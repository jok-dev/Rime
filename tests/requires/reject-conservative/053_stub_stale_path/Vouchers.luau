--!rime
local Vouchers = {}

function Vouchers.give(player: string, voucherId: string): boolean
	print(player, voucherId)
	return true
end

return table.freeze(Vouchers)
