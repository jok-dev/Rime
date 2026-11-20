--!rime
--!no-exec
-- No `table.freeze` on the dep's return — modules are treated as sealed
-- regardless (decided 2026-08-22): a static, non-escaping export is closed,
-- so the missing member is still flagged.
local Economy = require(script.Parent.dep)

Economy.missing()
--~ ERROR[R0306]: `missing` is not a member of module `dep`

-- The real member is used, so R0309 (unused export) stays out of this case.
Economy.grant("sam")

return table.freeze({})
