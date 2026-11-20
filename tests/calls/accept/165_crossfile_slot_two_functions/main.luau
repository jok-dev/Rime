--!rime
--!no-exec
-- A slot of a required module that is given two functions has no one
-- signature: which function a call reaches depends on when it runs. No call
-- through it is checked from another file. It is still a member, and the
-- module's other functions are checked as usual.
local Mailer = require(script.Parent.Mailer)

Mailer.connect("[mail]")
Mailer.send("hello", 2)
Mailer.send()
Mailer.close("done")

return table.freeze({})