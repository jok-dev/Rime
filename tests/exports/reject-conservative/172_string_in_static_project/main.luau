--!rime
-- Every require in this project is static, so a string cannot reach a
-- member unseen: looking one up by name takes `Mod[name]` or passing the
-- module along, and both already silence the whole module. Here neither
-- happens, so the string "refresh" proves nothing: `refresh` is reported
-- with no mark.
local main = {}

main.used = 1
main.hooks = { "refresh" }

function main.refresh() end
--~ WARNING[R0309]: export `refresh` of module `main` is never used

return main
