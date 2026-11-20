--!rime
-- A static require path into a subtree the project does not mount. Rime
-- cannot verify it, and unverifiable-at-check-time fails by default.
-- It has its own code (R0503) so exactly this class can be
-- downgraded when a subtree is managed outside Rojo.
--
-- The calls THROUGH the unverified binding must stay silent — no interface
-- means no R03xx claims (the snapshot proves only R0503 fires).
local Unknown = require(game.ServerStorage.DoesNotExist)
--~ ERROR[R0503]: require outside the project's view

Unknown.whatever(1, 2, 3)

return table.freeze({ Unknown = Unknown })
