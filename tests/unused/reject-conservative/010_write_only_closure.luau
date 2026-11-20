--!rime
-- A local only ever WRITTEN — including writes from nested closures — is
-- dead state: nothing observes it. Write-only capture stopped counting as
-- use (2026-08-23); a read from any closure still counts (accept/013), and
-- a compound assign (`x += 1`) reads before writing, so it still silences.
local M = {}

local lastNotified = 0
--~ WARNING[R0013]: `lastNotified` is assigned, but never read
--~ WARNING[R0016]: value assigned to `lastNotified` is never read

function M.notify()
	lastNotified = os.clock()
end

function M.reset()
	lastNotified = 0
end

return M
