--!rime
local Maid = {}

function Maid.New(): { destroy: () -> () }
	return { destroy = function() end }
end

return table.freeze(Maid)
