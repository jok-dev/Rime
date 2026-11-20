--!rime
-- The required module. `send` is given one function, and `shutdown` clears
-- the slot.
local Mailer = {}

function Mailer.send(message: string)
	print(message)
end

function Mailer.shutdown()
	-- rime: allow(exported-function-reassign, "nothing may send after shutdown")
	Mailer.send = nil
end

return Mailer