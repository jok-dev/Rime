--!rime
-- The same guard shape with a real consumer afterwards: the read at the
-- return is external, so the binding is not faint and the fallback store
-- is genuinely live. Silence from the whole unused family.
local function greeting(names: { [number]: string }, id: number): string
	local name = names[id]
	if not name then
		name = "guest"
	end
	return "hello, " .. name
end

return table.freeze({ greeting = greeting })
