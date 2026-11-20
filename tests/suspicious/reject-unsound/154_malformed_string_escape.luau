--!rime
-- An escape sequence Luau refuses (R0611, Selene's `bad_string_escape`).
--
-- WHY REJECTED: the file does not compile ("String literal contains
-- malformed escape sequence"). A decimal escape is one byte, `\x` takes
-- exactly two hexadecimal digits, and `\u` takes braces around a code
-- point no higher than U+10FFFF.
-- WHAT WOULD FIX IT: `\u{12C}` for the code point 300, `\x0A`, `\u{1F600}`.
--~ ERROR[R0611]: decimal escape `\300` is above 255
local tooHigh = "\300"
--~ ERROR[R0611]: `\x` escape without two hexadecimal digits
local shortHex = "\xA"
--~ ERROR[R0611]: `\u` escape without braces
local noBraces = "\u1F600"
--~ ERROR[R0611]: `\u{110000}` is above the last code point, U+10FFFF
local pastTheEnd = `\u{110000}`

return table.freeze({ tooHigh, shortHex, noBraces, pastTheEnd })