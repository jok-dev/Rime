--!rime
-- Case 93: the bare declaration is reachable — when `c` is
-- false nothing ever writes `reach`, so it IS nil at the use. A claimed
-- nil on one path outranks an opaque write on the other: firm report.
local function reachHeight(c: boolean, h: () -> Vector3?): number
	local reach: Vector3?
	if c then
		reach = h()
	end
	return reach.Y
	--~ ERROR[R0201]: `reach` may be nil
end

return table.freeze({ reachHeight = reachHeight })
