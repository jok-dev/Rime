--!rime
-- `BindToRenderStep` callbacks run inside the frame pipeline — a yield
-- there silently misbehaves (frames skip the step; work lands on the wrong
-- frame). Same rule as `Changed`, different registration site.
--
-- NOT IMPLEMENTED YET (needs the yield-effect pass): stays red as spec.
local RunService = game:GetService("RunService")

local function bind(target: Instance): ()
	RunService:BindToRenderStep("Track", 200, function()
		local hit = target:WaitForChild("Hitbox")
		--~ ERROR[R0204]: handler cannot yield
		print(hit)
	end)
end

return table.freeze({ bind = bind })
