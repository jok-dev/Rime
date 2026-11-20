--!rime
-- Empty if arm (R0701, selene parity 2026-08-21).
--
-- WHY REJECTED: an empty arm runs nothing — the logic is missing, or the
-- condition should fold into the arms that remain. Rime does not read
-- comments, so a comment-only arm still counts as empty.
-- WHAT WOULD FIX IT: fill the arm, drop it, or invert the condition.
local function reactTo(health: number): string
	--~ WARNING[R0701]: the `then` arm of this `if` is empty
	if health > 50 then
	else
		return "hurt"
	end
	return "fine"
end

return table.freeze({ reactTo = reactTo })
