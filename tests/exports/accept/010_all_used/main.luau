--!rime
-- Every export is consumed: value field read, dot call, method call.
local main = {
	limit = 4,
}

function main.get(): number
	return main.limit
end

function main:reset()
end

return table.freeze(main)
