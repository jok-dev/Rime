local helper = require("@Shared/helper")

local task = {}

function task.run(player: string): boolean
	return helper.grant(player, 1, "task")
end

return table.freeze(task)
