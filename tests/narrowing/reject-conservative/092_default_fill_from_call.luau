--!rime
-- Case 92: the default-fill-from-call idiom. `reach` is
-- declared `Vector3?` because the first source can return nil; the
-- `if reach == nil` fallback assigns a call result, and Rime cannot type
-- call results yet, so the fallback proves nothing. The use is reported,
-- but CONSERVATIVELY: no path claims nil, Rime just cannot classify the
-- value it holds.
--
-- WHY REJECTED: `value_nil` is syntactic; `g()` is opaque.
-- WHAT WOULD FIX IT: call-return nil-ness (declared return types, Roblox
-- API returns), or bind the value to a local declared without `?`.
local function reachHeight(c: boolean, g: () -> Vector3, h: () -> Vector3?): number
	local reach: Vector3?
	if c then
		reach = h()
	end
	if reach == nil then
		reach = g()
	end
	return reach.Y
	--~ ERROR[R0201]: `reach` may be nil
end

return table.freeze({ reachHeight = reachHeight })
