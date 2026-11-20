--!rime
-- A handler that yields TRANSITIVELY: it calls a same-file function whose
-- body yields. The effect propagates bottom-up through the local call
-- graph — hiding the `task.wait` one call away must not launder it.
--
-- NOT IMPLEMENTED YET (needs the yield-effect pass): stays red as spec.
local function fadeOut(part: Instance): ()
	task.wait(1)
	part.Parent = nil
end

local function watch(part: Instance): ()
	part.Changed:Connect(function()
		fadeOut(part)
		--~ ERROR[R0204]: handler cannot yield
	end)
end

return table.freeze({ watch = watch })
