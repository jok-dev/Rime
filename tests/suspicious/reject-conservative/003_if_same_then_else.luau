--!rime
-- Branch identical to the else fallback (R0603, selene parity 2026-08-21).
--
-- WHY REJECTED: the arm runs exactly what the `else` fallback would run, so
-- either the branch was meant to differ (copy-paste slip) or its condition
-- is unnecessary. Two CONDITIONAL arms sharing a body stay legal — that is
-- the deliberate multi-condition dispatch idiom (see the accept case).
-- WHAT WOULD FIX IT: make the branch differ, or drop the condition.
local function bump(flag: boolean, n: number): number
	--~ ERROR[R0603]: this branch matches the `else` branch
	if flag then
		return n + 1
	else
		return n + 1
	end
end

return table.freeze({ bump = bump })
