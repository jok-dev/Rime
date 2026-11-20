--!rime
-- The indexer is on a FIELD's type, not on the export's own type.
local inner: { byName: { [string]: number } } = { byName = {} }

return table.freeze(inner)
