--!rime
-- Case 94: `local x: T? = call()`. The annotation is the
-- user's own claim that THIS value may be nil (why else write `?`), so the
-- unguarded use is a firm report even though the value came from a call.
local function height(h: () -> Vector3?): number
	local reach: Vector3? = h()
	return reach.Y
	--~ ERROR[R0201]: `reach` may be nil
end

return table.freeze({ height = height })
