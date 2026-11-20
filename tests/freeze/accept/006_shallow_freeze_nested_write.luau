--!rime
-- Case 6 (plan §9): table.freeze is SHALLOW. `outer` is frozen but the table
-- in `outer.inner` is not, so writing one level down is legal at runtime —
-- through either binding. Only a single-level write (`outer.x = 1`) hits the
-- frozen table itself. (FrozenState::Deep exists for the later
-- DeepFrozen-by-type rule; nothing produces it yet.)
local inner = { count = 0 }
local outer = table.freeze({ inner = inner })
outer.inner.count = 1
inner.count = 2

return outer
