--!rime
--!no-exec
-- A file that rebinds `require` to its own loader. A call through the new
-- binding is not the builtin: the loader decides what a name means, so
-- there is nothing for Rime to resolve and nothing to report. The builtin
-- call on the right-hand side still resolves: the new local is not in
-- scope until its statement ends.
local require = require(script.Parent.loader)

local Alpha = require("Alpha")
local Beta = require("Beta")

return table.freeze({ Alpha = Alpha, Beta = Beta })
