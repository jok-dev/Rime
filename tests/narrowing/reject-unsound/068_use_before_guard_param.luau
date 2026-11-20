--!rime
-- R0202: the guard comes AFTER the use. `entry` is indexed on the first
-- line of the body, so a nil `entry` throws there and `if not entry` never
-- runs — the check protects nothing. The parameter is deliberately
-- unannotated (the shape real code has): the contradiction between the
-- index and the check is the evidence, not the type.
-- WHAT WOULD FIX IT: move the check above the first use.
local function describe(entry): string
	local words = { entry.label, entry.kind }
	if not entry then
		--~ ERROR[R0202]: `entry` is checked for nil after it was already indexed
		return "missing"
	end
	return table.concat(words, " ")
end

return table.freeze({ describe = describe })
