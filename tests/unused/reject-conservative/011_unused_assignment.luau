--!rime
-- Rust parity (`unused_assignments`, decided 2026-08-23): every store to a
-- never-read binding is dead, and each store is its own occurrence —
-- R0013 names the binding ("is assigned, but never read" once stores
-- exist), R0016 marks each write site, closure writes included. Deleting
-- just the `local` line strands the writes as R0023 global writes;
-- flagging every site shows the full footprint up front.
local M = {}

local attempts = 0
--~ WARNING[R0013]: `attempts` is assigned, but never read
attempts = 1
--~ WARNING[R0016]: value assigned to `attempts` is never read

local lastNotified = 0
--~ WARNING[R0013]: `lastNotified` is assigned, but never read

local function touch()
	lastNotified = os.clock()
	--~ WARNING[R0016]: value assigned to `lastNotified` is never read
end

function M.reset()
	lastNotified = 0
	--~ WARNING[R0016]: value assigned to `lastNotified` is never read
end

M.touch = touch

return M
