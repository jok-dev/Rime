--!rime
--~ OPTION[R0312]: on
-- Declarations are not work: locals, literals, operators, indexing,
-- function definitions, types, the return.

type Entry = { name: string, weight: number }

local WIDTH = 16
local HEIGHT = WIDTH * 2 + 1
local TITLE = "panel" .. "-" .. WIDTH

local Catalog = {}
Catalog.Entries = {
	{ name = "first", weight = 1 },
	{ name = "second", weight = HEIGHT },
} :: { Entry }
Catalog.Default = Catalog.Entries[1]
Catalog.Title = if WIDTH > 8 then TITLE else "small"

local function weigh(entry: Entry): number
	return entry.weight * 2
end

function Catalog.Total(): number
	local total = 0
	for _, entry in Catalog.Entries do
		total += weigh(entry)
	end
	return total
end

return Catalog
