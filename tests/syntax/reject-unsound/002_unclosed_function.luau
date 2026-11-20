--!rime
-- A function body that is never closed. full_moon only knows where parsing
-- stopped (the end of the file); Rime's own token-level block matcher names
-- the block that is still open, so the fix is one click away instead of a
-- hunt through 800 lines.
--~ ERROR[R0900]: could not parse this file as Luau: unexpected end of file
local M = {}

function M.closed()
	return 1
end

function M.open()
	if true then
		return 2
	end

return M
