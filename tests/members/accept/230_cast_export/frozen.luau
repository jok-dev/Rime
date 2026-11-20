--!rime
-- The cast wraps a frozen return.
type Registry = { [string]: number }

local frozen = { known = 1 }

return table.freeze(frozen) :: Registry
