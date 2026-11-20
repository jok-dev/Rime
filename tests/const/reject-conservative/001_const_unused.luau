--!rime
-- A `const` nobody reads is the same leftover a `local` nobody reads is:
-- R0013 at the declaration for a const value, R0015 for a
-- `const function` (as for a `local function`).
--
-- WHY REJECTED: valid Luau, but Rime cannot tell leftover clutter from a
-- mistyped name.
-- WHAT WOULD FIX IT: delete the binding, or prefix it with `_`.
local M = {}

const RETRIES = 3
--~ WARNING[R0013]: `RETRIES` is never read

const function describe(): string
--~ WARNING[R0015]: `describe` is never used
	return "unused"
end

function M.run(): number
	const scratch = 1
	--~ WARNING[R0013]: `scratch` is never read
	return 0
end

return table.freeze(M)
