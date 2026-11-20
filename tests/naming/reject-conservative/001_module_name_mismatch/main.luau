--!rime
-- The exported table's local must be named after its file: this file is
-- `main.luau`, so the module local should be `main`.
local M = {
	a = 1,
}
--~ WARNING[R0310]: module local `M` should be named `main`
return M
