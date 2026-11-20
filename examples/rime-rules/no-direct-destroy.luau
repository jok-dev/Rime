-- rime: allow-file(undefined-variable, "rule files run inside Rime's rule VM, where `rime` is a global")
-- Pooled instances must be handed back with Pool.release, which also takes
-- them off the pool's books; a bare :Destroy() leaves the pool holding a
-- dead instance. Presence check: walk every call, match by callee name.
--
-- The receiver-name test ("pooled" anywhere in the callee) is deliberately
-- loose, and will report some calls that are fine. Tighten
-- `name:lower():find(...)` to your naming if it is noisy.
return {
	severity = "warn",
	files = { "src/**/*.lua", "src/**/*.luau" },
	check = function(file, ctx)
		if file.path:match("Pool%.lua$") then
			return
		end
		for _, call in rime.calls(file.root) do
			if call.method == "Destroy" and #call.args == 0 then
				local name = rime.callee_name(call) or ""
				if name:lower():find("pooled", 1, true) then
					ctx:fail(call, "pooled instance destroyed directly — use Pool.release")
				end
			end
		end
	end,
}
