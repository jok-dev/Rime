--!rime
-- `GetPropertyChangedSignal` handlers have the same no-yield constraint as
-- `Changed` — the chained `:Connect` registration must be seen through.
--
-- NOT IMPLEMENTED YET (needs the yield-effect pass): stays red as spec.
local function watch(part: Instance): ()
	part:GetPropertyChangedSignal("Anchored"):Connect(function()
		part.AssemblyLinearVelocity = Vector3.zero
		task.wait(0.1)
		--~ ERROR[R0204]: handler cannot yield
		part.Parent = nil
	end)
end

return table.freeze({ watch = watch })
