--!rime
-- Companion to case 26: the trailing `error(...)` diverges, so no path falls
-- off the end. The guard-then-error shape is a very common way for an
-- annotated function to end without a final `return`.
local function parsePort(raw: string): number
	local n = tonumber(raw)
	if n ~= nil then
		return n
	end
	error("not a number: " .. raw, 0)
end

return table.freeze({ parsePort = parsePort })
