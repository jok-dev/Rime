--!rime
-- Calls on the module's own export that R0306 leaves alone, because the
-- member is defined somewhere in this file:
--   `second`, `third`  further down than the caller
--   `late`             only inside another function, on one branch
--   `callback`         a value member — what it holds is not modelled
--   `fromTable`        in the table the export is declared with
local main = {
	fromTable = function()
		return 4
	end,
}

main.callback = print

function main.first()
	return main.second() + main:third() + main.fromTable()
end

function main.second()
	return 1
end

function main:third()
	return 2
end

function main.setup(flag: boolean)
	if flag then
		main.late = function()
			return 3
		end
	end
end

function main.useLate()
	main.callback("late")
	return main.late()
end

return main
