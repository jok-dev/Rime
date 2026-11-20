--!rime
-- Every write to a never-localized name is its own diagnostic — editors
-- jump to a diagnostic's primary file:line, so each site must carry one
-- (dedup-to-first and packed secondary labels were both tried and
-- rejected). The count note connects the sites.
local Players = {}

local function find(model, userId)
	player = Players[model]
	if not player then
		player = Players[userId]
	end
	return player
end

return find
--~ ERROR[R0023]: global write: `player` is not declared `local`
