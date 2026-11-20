--!rime
-- Every escape Luau knows (R0611 and R0707 stay silent), and backslashes
-- that are not escapes at all because the string is a long string.
local named = "\a\b\f\n\r\t\v\\"
local quotes = "say \"hi\"" .. 'it\'s' .. `tick \` braces \{\} quotes \"\'`
local bytes = "\65\065\255\x41\x7f"
local points = "\u{41}\u{10FFFF}\u{0000000000000041}"
local continued = "one \
two \z
       three"
local raw = [[C:\Users \d \300]] .. [==[\x \u]==]
local fourDigits = "\2555" -- `\255` then a `5`
local threeHex = "\x414" -- `\x41` then a `4`

return table.freeze({ named, quotes, bytes, points, continued, raw, fourDigits, threeHex })