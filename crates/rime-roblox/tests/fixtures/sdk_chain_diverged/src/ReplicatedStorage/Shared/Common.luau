-- A module under a ReplicatedStorage mount with no script-class stem, so
-- `context_of` is BOTH: the same require runs on the server AND the client,
-- and the two branches of `locateModules` land on two different files.
local Sdk = require(game.ReplicatedStorage:WaitForChild("Packages"):WaitForChild("SdkShared"))

local Flags = require(Sdk.Modules.Flags)

return Flags
