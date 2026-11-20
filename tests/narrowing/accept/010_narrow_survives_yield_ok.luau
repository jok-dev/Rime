--!rime
--!no-exec
-- Case 10, REVERSED from plan §9 by a later decision (2026-08-21): an
-- UNCAPTURED local's nil-narrowing survives yields and calls. A local with
-- no capturing closure cannot be rebound by anything — that is language
-- semantics, not effect analysis — so the guard still holds after the wait.
--
-- What yields DO invalidate is the world around a handle (a destroyed
-- Instance is stale, not nil) — that is case 15's rule, not narrowing.
-- Captured locals still die (case 14); frozen-across-yield for CAPTURED
-- state still waits for Deep (case 11).
-- (`--!no-exec`: `task.wait` is not in vanilla Luau.)
local function fade(part: Instance?): ()
	if part ~= nil then
		task.wait(1)
		part:Destroy()
	end
end

return table.freeze({ fade = fade })
