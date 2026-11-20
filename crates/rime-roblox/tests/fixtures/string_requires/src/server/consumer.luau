local alpha = require("../shared/alpha")
local util = require("@Util")
local helper = require("@shared/util/helper")
local byName = require("Registry")
local kit = require("@nowhere/kit")

local consumer = {}

function consumer.run(player: string): boolean
	util.grant(player, 1, "util")
	helper.grant(player, 2, "helper")
	return alpha.grant(player, 3)
end

consumer.byName = byName
consumer.kit = kit

return table.freeze(consumer)
