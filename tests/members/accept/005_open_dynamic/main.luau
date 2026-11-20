--!rime
--!no-exec
-- `gamma` may or may not exist — the interface is open (dynamic writes in
-- the dep), so no membership claim is possible. Silent.
local Registry = require(script.Parent.dep)

Registry.gamma()

return table.freeze({})
