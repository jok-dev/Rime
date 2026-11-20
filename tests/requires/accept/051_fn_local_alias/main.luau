--!rime
--!no-exec
-- A require through a FUNCTION-LOCAL alias must still resolve: service and
-- parent aliases live inside handlers in real code
-- (`local RS = game:GetService(...)` in a provide() function, which was
-- once a false R0502). The alias env is module-wide; a name bound
-- more than once anywhere is dropped instead of guessed.
local function get(): number
	local P = script.Parent
	local Economy = require(P.dep)
	return Economy.balance("sam")
end

return table.freeze({ get = get })
