--!rime
-- Module-scope (top-level) bindings are checked since v2 (2026-08-23): an
-- unused top-level local, require, or function is the same clutter/typo
-- signal as inside a function. The export local (`M`, read by `return M`)
-- stays silent, and type-only requires stay silent via `Module::type_refs`
-- (accept/012).
local M = {}

local UNUSED_LIMIT = 3
--~ WARNING[R0013]: `UNUSED_LIMIT` is never read

local function helper()
	return 1
end
--~ WARNING[R0015]: `helper` is never used

function M.run()
	return 2
end

return M
