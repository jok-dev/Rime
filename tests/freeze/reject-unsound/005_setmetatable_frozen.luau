--!rime
-- Case 5 (plan §9): setmetatable counts as a write, so it throws on a frozen
-- table — class construction must set the metatable first and freeze LAST.
local Registry = { __index = {} }
local config = table.freeze({ volume = 5 })
setmetatable(config, Registry)
--~ ERROR[R0042]: cannot set a metatable on a frozen table
--~ NOTE: frozen at line 5

return config
