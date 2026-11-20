--!rime
-- NOT R0202: `node` is reassigned between the index and the check, so the
-- check tests a different value. A write resets the rule's memory of the
-- earlier use.
local function second(node): string
	local first = node.name
	node = node.next
	if not node then
		return first
	end
	return node.name
end

return table.freeze({ second = second({ name = "a", next = { name = "b" } }) })
