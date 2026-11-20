-- The client half of the chain. A module under ReplicatedStorage is shared,
-- and reaches the client branch of `locateModules` only as half of a union;
-- a `.client` stem is how the fixture exercises that branch on its own.
-- The require goes through a `game:GetService` alias.
local ReplicatedStorage = game:GetService("ReplicatedStorage")

local Sdk = require(ReplicatedStorage.Packages.SdkShared)

local Flags = require(Sdk.Modules.Flags)

return Flags
