--!rime
local M = {}
local function get()
	if math.random() > 0.5 then
		return script.Parent.A
	end
	return script.Parent.B
end
M.Sub = get()
return M
