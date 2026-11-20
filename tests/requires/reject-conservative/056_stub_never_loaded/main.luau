--!rime
-- A `false and require(...)` stub whose path RESOLVES — but no live
-- assignment of the variable exists anywhere in the file. The lazy-require
-- idiom is incomplete: nothing ever loads the module, so `Economy` is
-- `false` forever and every guarded branch is dead. Distinct from 054
-- (broken path): here the module exists — the file just never loads it.
--
-- WHY CONSERVATIVE: `false and require` doubles as a deliberate
-- kill-switch (ship the wiring, disable the feature); Rime cannot read
-- intent, so this is downgradeable like the rest of the family.
local Economy = false and require(script.Parent.dep)
--~ ERROR[R0506]: never loaded

local function balance()
	if Economy then
		return Economy.balance("sam")
	end
	return 0
end

return table.freeze({ balance = balance })
