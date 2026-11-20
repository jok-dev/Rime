-- The shared half of a two-package SDK, written for this fixture. What the
-- export-value evaluator has to get through before a consumer's
-- `require(Sdk.Modules.Flags)` resolves:
--   * a context test read from RunService, true on the server only
--   * a function of this file called from the returned table's
--     constructor, which calls another
--   * an `or` fallback between two instance paths
--   * a nil guard that returns early
--   * a require whose target is computed, chased through a linker module
--     into a package that returns its own instance
--   * the result stored as a named field of the table this module returns
--   * later writes into that table under computed names
local RunService = game:GetService("RunService")
local ServerStorage = game:GetService("ServerStorage")

-- A shared package cannot depend on a server one, so the server half is
-- found by where the project mounts its packages.
local function serverHalf(): Instance?
  local mount = ServerStorage:FindFirstChild("ServerPackages") or ServerStorage:FindFirstChild("Packages")
  if not mount then
    return nil
  end
  return require(mount.Sdk)
end

-- The modules sit in the server package on the server and beside this file
-- on the client.
local function locateModules(): Instance?
  if not RunService:IsServer() then
    return script.Modules
  end
  local half = serverHalf()
  if half then
    return half.Modules
  end
  return nil
end

local Sdk = {
  Version = "1.0.0",
  Modules = locateModules(),
}

-- Everything else beside this file can be reached by name as well.
for _, child in script:GetChildren() do
  if Sdk[child.Name] == nil then
    Sdk[child.Name] = child
  end
end

return Sdk
