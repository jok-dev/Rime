--!rime
-- Closed module: table literal export, static function slots, frozen return.
local dep = {}

function dep.grant(player: string, amount: number): boolean
	print(player, amount)
	return true
end

return table.freeze(dep)
