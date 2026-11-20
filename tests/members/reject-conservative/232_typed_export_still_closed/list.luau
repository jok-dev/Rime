--!rime
-- A number indexer (and the `{ T }` array form) says nothing about string
-- keys: no named member can be there.
local list: { [number]: string } = { "a", "b" }

return table.freeze(list)
