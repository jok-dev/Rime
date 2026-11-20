--!rime
-- Degrade mode (loader.luau requires dynamically). The string "tick" is
-- written on five lines of the project: three in consumer.luau, two here.
-- The report for `tick` names the first three and counts the rest;
-- `--format json` lists all five. Two strings on one line are one place.
local main = {}

main.used = 1
main.order = { "tick", "tock", "tick" }
main.first = "tick"

function main.tick() end
--~ WARNING[R0309]: export `tick` of module `main` is never used

return main
