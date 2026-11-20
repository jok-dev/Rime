-- Under ReplicatedStorage, so this module can run on a client. The only
-- `Vault` in the project is under ServerScriptService, which a client
-- cannot see: writing that path here would trade one broken require for
-- another.
local ReplicatedStorage = game:GetService("ReplicatedStorage")

local Vault = require(ReplicatedStorage.Vault)

local Hud = {}

function Hud.show()
	return Vault
end

return Hud