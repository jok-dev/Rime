--!rime
--!no-exec
-- It is the indexer that opens the export, whatever its value type.
local Indexed = require(script.Parent.indexed)

print(Indexed.known, Indexed.filledLater)
Indexed.alsoLater()

return table.freeze({})
