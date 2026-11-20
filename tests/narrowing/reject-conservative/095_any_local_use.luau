--!rime
-- Case 95: `any` admits nil, but nobody writes `: any` to
-- say "this may be nil" — it is a type-system opt-out. An `any` annotation
-- never upgrades an unclassifiable write into a nil claim, so such writes
-- stay opaque, reported conservatively, never as a firm claim.
--
-- WHAT WOULD FIX IT: a real annotation — `{ x: number }?` plus a guard, or
-- `{ x: number }` if it is never nil.
local function read(get: () -> any): number
	local data: any = get()
	return data.x
	--~ ERROR[R0201]: `data` may be nil
end

return table.freeze({ read = read })
