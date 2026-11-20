--!rime
-- The same behind an alias, and as part of a union.
type Loose = any

local alias: Loose | string = {}

alias.known = 1

return alias
