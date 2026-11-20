--!rime
--!no-exec
-- UDim2.new with fewer than four arguments (R0804, selene parity
-- `roblox_suspicious_udim2_new`, 2026-11-20).
--
-- WHY REJECTED: `UDim2.new` takes xScale, xOffset, yScale, yOffset. With
-- two numbers it sets only the X axis — `UDim2.new(0.5, 0.5)` is half the
-- parent's width plus half a pixel, and no height at all. The call runs
-- (the missing arguments are 0), so nothing says the Y axis was dropped.
-- WHAT WOULD FIX IT: `UDim2.fromScale(0.5, 0.5)` for a scale per axis,
-- `UDim2.fromOffset(200, 50)` for pixels per axis, or all four arguments.
local function centered(): UDim2
	--~ WARNING[R0804]: `UDim2.new` takes 4 numbers, but 2 were given
	return UDim2.new(0.5, 0.5)
end

local function button(): UDim2
	--~ WARNING[R0804]: `UDim2.new` takes 4 numbers, but 2 were given
	return UDim2.new(200, 50)
end

local function fullWidth(height: number): UDim2
	--~ WARNING[R0804]: `UDim2.new` takes 4 numbers, but 3 were given
	return UDim2.new(1, 0, height)
end

return table.freeze({ centered = centered, button = button, fullWidth = fullWidth })
