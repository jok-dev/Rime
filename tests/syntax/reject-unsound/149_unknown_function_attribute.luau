--!rime
-- A function attribute Luau does not know, or one given twice (R0044,
-- selene parity `unknown_function_attribute`, 2026-11-20).
--
-- WHY REJECTED: Luau refuses to compile the file: "Invalid attribute
-- '@inline'", "Cannot duplicate attribute '@native'". The attributes it
-- knows are `@native`, `@checked` and `@deprecated`, on every function
-- form: a declaration, a local function, a function expression.
-- WHAT WOULD FIX IT: remove the attribute, or spell the one that was meant.
local M = {}

@inline
--~ ERROR[R0044]: unknown function attribute `@inline`
function M.fast(a: number, b: number): number
	return a + b
end

@Native
--~ ERROR[R0044]: unknown function attribute `@Native`
local function shout(text: string): string
	return string.upper(text)
end

M.shout = shout

M.callback = @memoize function(n: number): number
	--~ ERROR[R0044]: unknown function attribute `@memoize`
	return n * 2
end

@native @native
--~ ERROR[R0044]: `@native` is given twice
function M.twice(n: number): number
	return n + n
end

return M