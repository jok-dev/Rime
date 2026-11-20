--!rime
-- Degrade mode (loader.luau requires dynamically). A value member that
-- holds its own name as a string does not mark its own report: the string
-- is part of the definition, not a use of it. That goes for a field of
-- the table the export is declared with (`Ready`), an assignment
-- (`Opened`) and a string deeper inside the value (`Closed`).
-- `Closed` is also written in `main.list`, another member: the report
-- for `Closed` is marked, with that line only.
local main = {
	used = 1,
	Ready = "Ready",
}
--~ WARNING[R0309]: export `Ready` of module `main` is never used

main.Opened = "Opened"
--~ WARNING[R0309]: export `Opened` of module `main` is never used

main.Closed = { name = "Closed" }
--~ WARNING[R0309]: export `Closed` of module `main` is never used

main.list = { "Closed" }

return main
