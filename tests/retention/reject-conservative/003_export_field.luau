--!rime
--!no-exec
-- Per-player map stored on the module's export table (R0803): the same
-- module-lifetime defect as 001, spelled `Module.field = {}`.
--
-- WHY REJECTED: `Mailbox.unread` gains an entry for every Player who is
-- sent a message and nothing here removes one. The sealed-module axiom
-- (R0308) says no other module may write it either, so the leak is total.
-- WHAT WOULD FIX IT: clear the entry on PlayerRemoving.
local Mailbox = {}

--~ WARNING[R0803]: `Mailbox.unread` is weak-keyed but holds its values for every Player ever added, and is never cleared in this module
Mailbox.unread = setmetatable({}, { __mode = "k" })

function Mailbox.deliver(to: Player, text: string)
	local queue = Mailbox.unread[to]
	if queue == nil then
		queue = {}
		Mailbox.unread[to] = queue
	end
	table.insert(queue, text)
end

function Mailbox.count(of: Player): number
	local queue = Mailbox.unread[of]
	return if queue then #queue else 0
end

return Mailbox
