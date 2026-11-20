--!rime
-- Comparison inside type() (R0606, selene parity 2026-08-21).
--
-- WHY REJECTED: `type(value == "number")` takes the type of a comparison's
-- boolean result — it always answers "boolean". The parenthesis slipped:
-- the intended check is `type(value) == "number"`.
-- WHAT WOULD FIX IT: move the comparison outside the call.
local function isNumber(value: unknown): boolean
	--~ ERROR[R0606]: the comparison is inside the `type` call
	return type(value == "number") == "boolean"
end

return table.freeze({ isNumber = isNumber })
