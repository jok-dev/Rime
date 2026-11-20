--!rime
-- Extra arguments to a local function, wherever it is declared and however
-- it got its value (R0304; Selene's `mismatched_arg_count`).
--
-- WHY REJECTED: Luau drops an argument the function has no parameter for.
-- Every function `send` or `log` can hold is in this file, and none takes
-- what these calls pass.
-- WHAT WOULD FIX IT: drop the argument, or give the function a parameter.
local send

local function connect(fast: boolean)
	if fast then
		send = function(message: string)
			print(message)
		end
	else
		send = function(message: string, retries: number)
			print(message, retries)
		end
	end
end

local function run(...: string)
	local function log(text: string)
		print(text)
	end
	--~ ERROR[R0304]: call to `log` has 2 arguments, but `log` takes 1
	log("start", "now")
	--~ ERROR[R0304]: call to `send` has 3 arguments, but no definition of `send` takes more than 2
	send("hello", 2, "urgent")
	--~ ERROR[R0304]: call to `log` has 2 arguments, but `log` takes 1
	log("rest", ...)
end

connect(true)
run("a")

return table.freeze({ connect = connect, run = run })