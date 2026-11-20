--!rime
--!no-exec
-- A member the closed interface does not contain is a theorem, not a
-- guess — the export is frozen and fully static, so `grantt` cannot exist.
local Economy = require(script.Parent.dep)

Economy.grantt("sam", 100)
--~ ERROR[R0306]: `grantt` is not a member of module `dep`

-- The real member is used, so R0309 (unused export) stays out of this case.
print(Economy.grant)

return table.freeze({})
