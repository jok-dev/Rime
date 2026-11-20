--!rime
local M = {}
local function get()
	if math.random() > 0.5 then
		return nil
	end
	return script.Parent.Sub
end
M.Sub = get()
return M
