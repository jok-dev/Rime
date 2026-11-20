--!rime
-- The indexer is behind an alias, declared after its use (aliases are
-- not ordered) and reached through a second alias.
local dep: Registry = {}

type Registry = Handlers
type Handlers = { [string]: () -> string }

function dep.known(): string
	return "known"
end

return dep
