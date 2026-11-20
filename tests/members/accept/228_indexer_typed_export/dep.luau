--!rime
-- The export's declared type has a string indexer: the type itself says
-- any string key may be present. What the file writes statically is not
-- the whole member set, so the interface is open.
local dep: { [string]: number } = {}

dep.known = 1

return dep
