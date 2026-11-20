--!rime
-- Exported-function-reassign (decided 2026-08-20): the exported
-- table's function slots are single-assignment.
--
-- WHY REJECTED: this late-binding runs fine today, but module returns are
-- require-cached and shared process-wide — swapping `M.greet` changes
-- behavior for every caller, and Rime cannot prove existing call sites stay
-- valid against the new function.
-- WHAT WOULD FIX IT: define the final function once, or route the varying
-- behavior through a local (`local greetImpl`), which is exempt on purpose.
local M = {}

function M.greet(): string
	return "hello"
end

M.greet = function(): string
	return "hi"
end
--~ ERROR[R0305]: cannot reassign exported function `M.greet`

return M
