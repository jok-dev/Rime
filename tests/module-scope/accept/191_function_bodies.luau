--!rime
--~ OPTION[R0312]: on
-- A function body is not module scope: nothing in it runs until it is
-- called. That holds for functions kept in tables and for function values
-- in a local.

local Queue = {}

local pending: { string } = {}

local function flush(): number
	local count = #pending
	table.clear(pending)
	return count
end

function Queue.Push(item: string)
	table.insert(pending, item)
	if #pending > 8 then
		flush()
	end
end

Queue.Drain = function(): number
	while #pending > 0 do
		table.remove(pending)
	end
	return flush()
end

Queue.Handlers = {
	onPush = function(item: string)
		print(string.upper(item))
	end,
}

return Queue
