--!rime
--!no-exec
-- `M.ready` was added through the escaped alias; `M.launch` genuinely does
-- not exist — but the interface is open, so both stay silent.
local M = require(script.Parent.dep)

print(M.ready)
M.launch()

return table.freeze({})
