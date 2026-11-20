--!rime
-- A declared type with named fields only: the member set is still exactly
-- what the file defines.
type Shape = { maxLevel: number }

local dep: Shape = { maxLevel = 100 }

return table.freeze(dep)
