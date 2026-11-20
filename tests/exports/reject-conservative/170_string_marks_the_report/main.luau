--!rime
-- Degrade mode (loader.luau requires dynamically). `refresh` and `reset`
-- are never read as members anywhere, but string literals in the project
-- equal their names: `refresh` in this file's own list, `reset` in
-- consumer.luau. Something reached through the dynamic require can look a
-- member up by such a name (`m[name]`), so the string may be a use. It may
-- also be a coincidence. Rime does not decide: both members are still
-- reported, and each report says where the string is, so the reader checks
-- before deleting and a tool can leave the report alone.
local main = {}

main.used = 1
main.hooks = { "refresh" }

function main.refresh() end
--~ WARNING[R0309]: export `refresh` of module `main` is never used

function main.reset() end
--~ WARNING[R0309]: export `reset` of module `main` is never used

return main
