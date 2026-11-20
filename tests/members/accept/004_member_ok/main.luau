--!rime
--!no-exec
-- Every member exists: calls, method call, value-field reads, and a
-- function referenced as a value. All silent.
local Shop = require(script.Parent.dep)

local ok = Shop.buy("apple")
Shop:restock()
print(Shop.maxSlots)
print(Shop.version)
local buyRef = Shop.buy

return table.freeze({ ok = ok, buyRef = buyRef })
