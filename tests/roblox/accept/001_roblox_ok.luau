--!rime
--!no-exec
-- Near-misses of the Roblox literal checks (R0801–R0802) that must stay
-- silent.
local function tints(): Color3
	local base = Color3.new(1, 0.5, 0) -- in range
	local bright = Color3.fromRGB(255, 128, 0) -- fromRGB is the 0–255 form
	return base:Lerp(bright, 0.5)
end

local function layouts(): UDim2
	-- These are why there is no check on the value in a UDim2 scale
	-- slot: every shape below is something code does on purpose.
	local half = UDim2.fromScale(0.5, 0.5) -- fractions: fine
	local pixels = UDim2.fromOffset(200, 50) -- pixels in the pixel form: fine
	local mixed = UDim2.new(0.5, 10, 1, -20) -- scales 0–1, offsets free
	local slide = UDim2.new(-2, 0, 0.25, 0) -- integer off-screen tween start
	local board = UDim2.new(6, 0, 3, 0) -- BillboardGui sizes are in studs
	return half + pixels + mixed + slide + board
end

return table.freeze({ tints = tints, layouts = layouts })
