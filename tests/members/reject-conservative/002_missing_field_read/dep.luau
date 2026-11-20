--!rime
-- Closed module with value fields: a literal entry and one static
-- top-level write, alongside a function slot.
local dep = {
	maxLevel = 100,
}
dep.version = "1.0"

function dep.reset()
	print("reset")
end

return table.freeze(dep)
