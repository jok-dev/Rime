--!rime
-- Empty loop body (R0702, selene parity 2026-08-21).
--
-- WHY REJECTED: a call-free loop with an empty body iterates and does
-- nothing — a busy-wait at best, dead code at worst. Loops whose HEADER
-- carries the effect (`while task.wait() do end`) stay silent.
-- WHAT WOULD FIX IT: give the body work, or delete the loop.
local function spin(flag: boolean): number
	--~ WARNING[R0702]: this `while` loop has an empty body
	while flag do
	end
	local count = 0
	--~ WARNING[R0702]: this numeric `for` loop has an empty body
	for _ = 1, 10 do
	end
	count += 1
	return count
end

return table.freeze({ spin = spin })
