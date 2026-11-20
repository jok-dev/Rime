--!rime
--!no-exec
-- `s` resolves through `resolve_loc` (Sub.luau exists), so `truth` proves
-- `s == nil` false and prunes the `return nil` arm (the evaluator's
-- nil-check narrowing) — the guard never contributes a Nil candidate.
local M = {}
local function get()
	local s = script.Parent.Sub
	if s == nil then
		return nil
	end
	return s
end
M.Sub = get()
return M
