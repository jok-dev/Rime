--!rime
-- Comparison inside type() on values that are more than a bare name (R0606).
--
-- WHY REJECTED: same slip as case 006 — the call takes the type of a
-- comparison's boolean result.
-- WHAT WOULD FIX IT: move the comparison outside the call. The suggested
-- edit keeps the value's own text (a field, a method call) and the
-- literal's own quoting, and replaces the whole call.
local function isReady(entry: { state: unknown }): boolean
	--~ ERROR[R0606]: the comparison is inside the `type` call
	return type(entry.state == 'string')
end

local function hasHandle(tool: Instance): boolean
	--~ ERROR[R0606]: the comparison is inside the `typeof` call
	return not typeof(tool:FindFirstChild("Handle", true) ~= "Instance")
end

return table.freeze({ isReady = isReady, hasHandle = hasHandle })
