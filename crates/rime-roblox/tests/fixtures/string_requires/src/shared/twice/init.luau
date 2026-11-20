local alpha = require('./alpha')

local twice = {}

function twice.run(player: string): boolean
	return alpha.grant(player, 1, 'twice')
end

return table.freeze(twice)
