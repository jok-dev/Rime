--!rime
-- One `end` short inside a nested `if`. The parser can only say the
-- function never closed; the indentation of the `end` that closed the
-- inner `if` says which block was really left open, and which block that
-- `end` was written for.
--~ ERROR[R0900]: could not parse this file as Luau: unexpected end of file
local function migrate(data)
	if data.version < 3 then
		if not data.ok then
			return
		else
		data.version = 3
	end
	return data
end
