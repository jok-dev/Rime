--!rime
--!no-exec
-- Cross-file calls that must stay silent: exact arity and trailing-optional
-- omitted. (Calls through an unverifiable require — and the require itself
-- failing — are requires/048's business.)
local Economy = require(script.Parent.dep)

local ok = Economy.grant("sam", 100, "quest")
local ok2 = Economy.grant("sam", 100)
local bal = Economy.balance("sam")

return table.freeze({ ok = ok, ok2 = ok2, bal = bal })
