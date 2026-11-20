--!rime
--!no-exec
-- Dynamic key: the constants-table pattern — a consumer populating a
-- required module at runtime. Sealed-module line: the write is the bug.
local Registry = require(script.Parent.dep)

local key = "cash"
Registry[key] = 100
--~ WARNING[R0308]: dynamic write to required module `dep`

return table.freeze({})
