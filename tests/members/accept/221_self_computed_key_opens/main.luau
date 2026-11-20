--!rime
--!no-exec
-- `Dep.logger` may well exist after `inject`. The interface is open, so
-- calls and reads of members the file does not name stay silent.
local Dep = require(script.Parent.dep)

Dep:inject({ logger = print })
Dep.logger("ready")
print(Dep.level)

return table.freeze({})
