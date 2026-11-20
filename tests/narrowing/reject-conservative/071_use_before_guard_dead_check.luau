--!rime
-- R0202, conservative flavour: `part` is annotated with a type that does
-- not admit nil, so the index cannot throw and the code is valid — but
-- the `if part == nil` that follows is dead. Rime still reports it: a
-- nil check that can never fire is a misunderstanding waiting to become a
-- bug when the type widens.
-- WHY REJECTED: the check contradicts both the annotation and the index
-- before it.
-- WHAT WOULD FIX IT: drop the check, or widen the type to `T?` and move
-- the check above the first use.
local function describe(part: { name: string }): string
	local name = part.name
	if part == nil then
		--~ ERROR[R0202]: `part` is checked for nil after it was already indexed
		return "none"
	end
	return name
end

return table.freeze({ describe = describe })
