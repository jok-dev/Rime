--!rime
--!no-exec
-- Near-misses of R0804 suspicious-udim2-new that must stay silent.
local function sizes(width: UDim, height: UDim, parts: { number }): UDim2
	local full = UDim2.new(1, 0, 1, 0) -- all four
	local empty = UDim2.new() -- no arguments: the zero UDim2
	local fromUDims = UDim2.new(width, height) -- the two-UDim overload
	local built = UDim2.new(UDim.new(0, 5), UDim.new(1, 0))
	local spread = UDim2.new(table.unpack(parts)) -- a call can supply all four
	local scale = UDim2.fromScale(0.5, 0.5)
	local offset = UDim2.fromOffset(200, 50)
	return full + empty + fromUDims + built + spread + scale + offset
end

local function forward(...: number): UDim2
	return UDim2.new(...)
end

return table.freeze({ sizes = sizes, forward = forward })
