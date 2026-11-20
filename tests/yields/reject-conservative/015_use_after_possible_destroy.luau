--!rime
-- Case 15 (plan §9, §3): an Instance handle used after a possible Destroy.
--
-- WHY REJECTED: on the `expired` path the part is destroyed when the write
-- runs — writing properties of a destroyed instance silently misbehaves (or
-- throws, depending on the property). May-analysis: only ONE path destroys,
-- so this is conservative, not a proven throw.
-- WHAT WOULD FIX IT: return early after `Destroy`, or restructure so the
-- destroy is last.
--
-- NOT IMPLEMENTED YET (needs instance-liveness tracking in the yield/effects
-- milestone): stays red as spec.
local function cleanup(part: Instance, expired: boolean): ()
	if expired then
		part:Destroy()
	end
	part.Name = "stale"
	--~ ERROR[R0203]: `part` may already be destroyed
end

return table.freeze({ cleanup = cleanup })
