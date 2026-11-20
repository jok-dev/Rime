-- Server context: only the server branch of `locateModules` applies.
local Packages = game:GetService("ReplicatedStorage"):WaitForChild("Packages")
local Sdk = require(Packages.SdkShared)

local Flags = require(Sdk.Modules.Flags)

return Flags
