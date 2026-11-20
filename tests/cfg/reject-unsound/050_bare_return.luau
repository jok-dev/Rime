--!rime
-- A bare `return` in a function whose annotation declares values: the
-- caller silently receives nil for each. Same contract violation as
-- falling off the end — but the diagnostic must point at the `return`
-- itself; pointing at the `end` sends the reader hunting through branches
-- that all return properly (as in `Update` of Roblox's stock LegacyCamera).
local function pick(flag: boolean): number
	if flag then
		return
		--~ ERROR[R0102]: `return` with no values
	end
	return 1
end

return table.freeze({ pick = pick })
