--!rime
-- The initializer is cast to a type with a string indexer.
type Registry = { [string]: number }

local dep = {} :: Registry

dep.known = 1

return dep
