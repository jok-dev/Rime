--!rime
--!no-exec
-- `const` (Luau RFC "const keyword") declares a binding that is scoped,
-- read and captured exactly like a `local`. Every name below resolves:
-- across statements, inside closures, in a `const function`'s own body
-- (recursion), and when one const's initializer reads an earlier one.
--
-- Static-only: the Luau vendored for the differential runner (0.663)
-- predates `const` and cannot execute this file.
const LIMIT = 10
const scale: number, offset: number = 2, 1
const DOUBLE_LIMIT = LIMIT * scale

const function clamp(n: number): number
	return math.min(n, LIMIT)
end

const function countdown(n: number): number
	if n <= 0 then
		return 0
	end
	return countdown(n - 1)
end

local M = {}

function M.apply(n: number): number
	return clamp(n * scale + offset)
end

function M.later(): () -> number
	return function()
		return DOUBLE_LIMIT + countdown(LIMIT)
	end
end

return table.freeze(M)
