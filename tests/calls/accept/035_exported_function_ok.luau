--!rime
-- The legal shapes around exported-function-reassign:
--   - `M.f = function` as the one declaration (common module style)
--   - swapping a LOCAL function variable (locals are exempt on purpose)
--   - reassigning a function field on a table that is NOT the export
local M = {}

M.greet = function(): string
	return "hello"
end

-- locals may rebind freely
local impl = function(): number
	return 1
end
impl = function(): number
	return 2
end

-- non-exported callback tables may hot-swap
local handlers = {}
handlers.onClick = function(): () end
handlers.onClick = function(): () end

function M.run(): number
	handlers.onClick()
	return impl()
end

return M
