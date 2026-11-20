--!rime
--!no-exec
-- Consumer population does NOT extend the interface (hard line, decided
-- 2026-08-22): the write is flagged, and the read of the runtime-populated
-- member is flagged too — Rime models the module as its own file defines it.
local Config = require(script.Parent.dep)

Config.injected = true
--~ WARNING[R0308]: write to `injected` on required module `dep`
print(Config.injected)
--~ ERROR[R0307]: `injected` is not a member of module `dep`

-- The real member is used, so R0309 (unused export) stays out of this case.
Config.get("coins")

return table.freeze({})
