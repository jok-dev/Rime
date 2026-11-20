--!rime
-- NOT R0202: `item.name` runs on one path only (`verbose` true). The check
-- after the join is legitimate on the other path, so the rule — a
-- must-analysis over paths — stays silent. (Whether the index itself is
-- safe is R0201's question, and an unannotated `item` makes no claim.)
local function label(item, verbose: boolean): string
	local prefix = ""
	if verbose then
		prefix = item.name .. ": "
	end
	if not item then
		return prefix .. "none"
	end
	return prefix .. "some"
end

return table.freeze({ label = label({ name = "x" }, true) })
