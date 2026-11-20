--!rime
--~ OPTION[R0312]: on
-- A function written as an argument is a value like any other. The call
-- that receives it keeps it, and its body runs when somebody calls it
-- later, so nothing in the body is module scope.

local function work(): number
	return 1
end

local handlers = {}
table.insert(handlers, function()
	return work()
end)

local byName = {}
rawset(byName, "work", function()
	return work()
end)

local packed = table.pack(function()
	return work()
end)

local filled = table.create(2, function()
	return work()
end)

local second = select(2, "first", function()
	return work()
end)

local kind = type(function()
	return work()
end)

local label = string.format("%s", tostring(function()
	return work()
end))

return {
	handlers = handlers,
	byName = byName,
	packed = packed,
	filled = filled,
	second = second,
	kind = kind,
	label = label,
}
