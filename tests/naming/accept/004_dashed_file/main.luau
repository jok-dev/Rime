--!rime
-- Dashes are stripped from the file stem before comparing: Car-V2.luau's
-- module local is `CarV2` and stays silent.
local main = {
	a = 1,
}
return table.freeze(main)
