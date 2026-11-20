--!rime
-- The required module. `send` starts as a stand-in and gets the real
-- function once `connect` has run: two functions in one slot.
local Mailer = {}

Mailer.send = function() end

function Mailer.connect(prefix: string)
	-- rime: allow(exported-function-reassign, "the stand-in is replaced once, on connect")
	Mailer.send = function(message: string, copies: number)
		print(prefix, message, copies)
	end
end

function Mailer.close(reason: string)
	print(reason)
end

return Mailer