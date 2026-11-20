--!rime
-- `...` read in a function that does not declare it (R0022, selene parity
-- `undefined_variable` on its `varargs` fixture, 2026-11-20).
--
-- WHY REJECTED: Luau refuses to compile the file: "Cannot use '...'
-- outside of a vararg function". `...` belongs to the function that
-- declares it and does not reach into a nested one, so a callback written
-- inside a variadic function has none of its own.
-- WHAT WOULD FIX IT: add `...` to the parameter list, or capture the
-- values outside (`local args = table.pack(...)`) and use the capture.
local M = {}

function M.plain()
	return ...
	--~ ERROR[R0022]: `...` is not defined in this function
end

function M.forward(...)
	return pcall(function()
		return M.plain(...)
		--~ ERROR[R0022]: `...` is not defined in this function
	end)
end

function M:count()
	return select("#", ...)
	--~ ERROR[R0022]: `...` is not defined in this function
end

return M