-- Every require here names a module that moved: each exists exactly once,
-- somewhere else in the project. `Car-V2` and `My Folder` are not
-- identifiers, so their paths need brackets. `Vault` is server-only, and so
-- is this file.
local ReplicatedStorage = game:GetService("ReplicatedStorage")

return {
	helper = require(ReplicatedStorage.Helper),
	car = require(ReplicatedStorage["Car-V2"]),
	spaced = require(ReplicatedStorage.Spaced),
	vault = require(ReplicatedStorage.Vault),
}
