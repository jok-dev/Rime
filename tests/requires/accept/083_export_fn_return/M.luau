--!rime
--!no-exec
-- The export field is written from a function call, not a bare
-- `script.Parent...` chain — the evaluator must walk into `get`'s own body
-- for its return-value candidates.
local M = {}
local function get()
	return script.Parent.Sub
end
M.Sub = get()
return M
