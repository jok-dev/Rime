--!rime
--!no-exec
-- Only bare-local (and freeze-wrapped) returns are checked: a constructed
-- return has no module local to name.
return setmetatable({}, {})
