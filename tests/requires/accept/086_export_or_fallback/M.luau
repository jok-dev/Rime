--!rime
--!no-exec
-- `FindFirstChild("Sub")` resolves (Sub.luau exists on disk), so the `or`
-- proves its lhs non-nil and short-circuits before ever evaluating the
-- fallback — `Fallback.luau` is deliberately absent from this case dir; a
-- non-nil lhs never touches the rhs (the evaluator's `or` handling).
local M = {}
M.Sub = script.Parent:FindFirstChild("Sub") or script.Parent.Fallback
return M
