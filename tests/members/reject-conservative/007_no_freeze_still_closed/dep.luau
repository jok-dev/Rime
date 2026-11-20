--!rime
-- No `table.freeze` on the return: any file holding this table can add
-- members at runtime, so the member set is not a theorem. OPEN.
local dep = {}

function dep.grant(player: string)
	print(player)
end

return dep
