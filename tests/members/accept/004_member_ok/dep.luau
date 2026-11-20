--!rime
-- Closed module mixing every member source the extractor models:
-- literal field, static top-level write, dot function, `:` method.
local dep = {
	maxSlots = 8,
}
dep.version = "1.0"

function dep.buy(item: string): boolean
	print(item)
	return true
end

function dep:restock()
	print("restock")
end

return table.freeze(dep)
