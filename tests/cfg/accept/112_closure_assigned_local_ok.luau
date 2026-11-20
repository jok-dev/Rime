--!rime
-- A local declared without a value and assigned by a closure. Whether the
-- closure has run by the time the local is read depends on when it is
-- called, which the per-function graph does not model, so from the point
-- the closure is created R0101 makes no claim about the local.
local function viaNamedClosure(): number
	local x: number
	local function set()
		x = 1
	end
	set()
	return x
end

local function viaCallback(run: (() -> ()) -> ()): number
	local total: number
	run(function()
		total = 10
	end)
	return total
end

local function viaNestedClosure(run: (() -> ()) -> ()): number
	local total: number
	run(function()
		run(function()
			total = 100
		end)
	end)
	return total
end

local function call(fn: () -> ())
	fn()
end

return table.freeze({
	named = viaNamedClosure(),
	callback = viaCallback(call),
	nested = viaNestedClosure(call),
})
