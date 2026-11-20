--!rime
-- A quote escaped where it needs no escape (R0707, the other half of
-- Selene's `bad_string_escape`).
--
-- WHY REJECTED: only the quote that delimits a string has to be escaped
-- inside it. The backslash on the other one changes nothing.
-- WHAT WOULD FIX IT: remove the backslash (each report carries the edit).
--~ WARNING[R0707]: `\'` needs no backslash in a double-quoted string
local double = "it\'s"
--~ WARNING[R0707]: `\"` needs no backslash in a single-quoted string
local single = 'say \"hi\"'

return table.freeze({ double, single })