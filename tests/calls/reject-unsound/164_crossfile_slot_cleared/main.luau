--!rime
--!no-exec
-- A slot of a required module that is given one function keeps its
-- signature for other files, whatever else the module stores in the slot.
--
-- WHY REJECTED: `Mailer.send` takes one argument. Clearing the slot in
-- `shutdown` stores no other function in it: a call after that throws, it
-- does not reach something that takes two.
-- WHAT WOULD FIX IT: drop the argument, or give `send` a parameter.
local Mailer = require(script.Parent.Mailer)

--~ ERROR[R0304]: call to `Mailer.send` has 2 arguments, but `Mailer.send` takes 1
Mailer.send("hello", 2)
Mailer.shutdown()

return table.freeze({})