--!rime
-- A backslash before a character that is no escape (R0611, Selene's
-- `bad_string_escape`).
--
-- WHY REJECTED: Luau drops the backslash and keeps the character, so the
-- string is not what it looks like. `"C:\Users"` is `C:Users`; `"\."`
-- is `.`, which in a pattern matches any character.
-- WHAT WOULD FIX IT: `\\` for a backslash; `%` to escape in a pattern.
--~ ERROR[R0611]: `\U` is not an escape sequence
local path = "C:\Users"
--~ ERROR[R0611]: `\.` is not an escape sequence
local version = string.match("1.2", "%d+\.%d+")
--~ ERROR[R0611]: `\d` is not an escape sequence
local digits = string.match("42", `^\d+$`)

return table.freeze({ path, version, digits })