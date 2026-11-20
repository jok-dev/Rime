--!rime
-- Case 16 (plan §9, §3): a yield inside a `Changed` handler.
--
-- Property-changed handlers (`Changed`, `GetPropertyChangedSignal`,
-- `BindToRenderStep` callbacks) must not suspend: the engine fires them on
-- a path where yielding silently misbehaves — events get dropped or
-- delivered out of order. Not conservative: the yield IS the bug class,
-- same reasoning as R0110.
--
-- NOT IMPLEMENTED YET (needs yield-effect tracking): stays red as spec.
local function watch(part: Instance): ()
	part.Changed:Connect(function()
		task.wait(1)
		--~ ERROR[R0204]: cannot yield inside a `Changed` handler
		print("changed")
	end)
end

return table.freeze({ watch = watch })
