--!rime
--!no-exec
-- Cross-file call conformance: the call is checked against the signature in
-- the file `require` resolves to. `reason` gets nil, lands in whatever the
-- callee does with it, and the stack blames dep.luau.
local Economy = require(script.Parent.dep)

Economy.grant("sam", 100)
--~ ERROR[R0301]: call to `dep.grant` is missing argument 3 `reason`

return table.freeze({})
