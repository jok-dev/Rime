local piece = require("./piece")

local legacy = {}

function legacy.run(player: string): boolean
	return piece.grant(player, 1, "legacy")
end

return table.freeze(legacy)
