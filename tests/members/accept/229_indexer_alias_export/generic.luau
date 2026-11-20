--!rime
-- A generic alias whose definition has the indexer.
type Map<V> = { [string]: V }

local generic: Map<number> = {}

generic.known = 1

return table.freeze(generic)
