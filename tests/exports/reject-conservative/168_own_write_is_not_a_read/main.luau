--!rime
-- Degrade mode (loader.luau requires dynamically): a member name read
-- anywhere in the project counts as used on every module. The module's own
-- `main.LIMIT = 5` DEFINES the member — it is not a read of it. Nothing in
-- the project reads `LIMIT` or `handler`, so both are flagged.
-- `main.total += 1` does read its slot, and consumer.luau reads `used`.
local main = {}

main.used = 1
main.LIMIT = 5
--~ WARNING[R0309]: export `LIMIT` of module `main` is never used
main.handler = function() end
--~ WARNING[R0309]: export `handler` of module `main` is never used
main.total = 0
main.total += 1

return main
