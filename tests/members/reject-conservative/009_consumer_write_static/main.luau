--!rime
--!no-exec
-- Required modules are sealed (decided 2026-08-22): a consumer must not
-- add or overwrite members — state belongs inside the owning module.
local Economy = require(script.Parent.dep)

Economy.bonus = 2
--~ WARNING[R0308]: write to `bonus` on required module `dep`

-- The real member is used, so R0309 (unused export) stays out of this case.
Economy.grant("sam")

return table.freeze({})
