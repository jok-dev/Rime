--!rime
-- Non-yielding handlers are legal: plain work, calls to local functions
-- that do not yield, and handing a yielding closure to `task.spawn` (a new
-- thread yields, the handler does not). The engine constraint is on the
-- handler's own thread only.
local function applyLook(part: Instance): ()
	part.Parent = workspace
end

local function watch(part: Instance): ()
	part.Changed:Connect(function()
		applyLook(part)
		task.spawn(function()
			task.wait(1)
			applyLook(part)
		end)
	end)
	part:GetPropertyChangedSignal("Name"):Connect(function()
		print("renamed")
	end)
end

return table.freeze({ watch = watch })
