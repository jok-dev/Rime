--!rime
-- Degrade mode (loader.luau requires dynamically). A string marks a
-- member's report only when it EQUALS the member's name. None of these do:
-- a different case, a longer name, a dotted path, a sentence that contains
-- the name, and a table key (`{ refresh = 1 }` defines a field of another
-- table, it is not a string). So `refresh` is reported with no mark.
local main = {}

main.used = 1
main.notes = { "Refresh", "refresh_all", "main.refresh", "call refresh later" }
main.other = { refresh = 1 }

function main.refresh() end
--~ WARNING[R0309]: export `refresh` of module `main` is never used

return main
