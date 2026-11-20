--!rime
--!no-exec
-- Field reads are membership-checked too: `maxLevl` is a typo of a
-- value field on a closed interface — it can only ever be nil.
local Config = require(script.Parent.dep)

print(Config.maxLevl)
--~ ERROR[R0307]: `maxLevl` is not a member of module `dep`

-- The real members are used, so R0309 (unused export) stays out of this case.
print(Config.maxLevel, Config.version, Config.reset)

return table.freeze({})
