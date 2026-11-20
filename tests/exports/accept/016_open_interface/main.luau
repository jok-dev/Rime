--!rime
-- The bare table escapes (`print(main)`), so the interface is open and
-- membership is unknowable — `b` may be read through the escaped alias.
local main = {}
main.a = 1
main.b = 2
print(main)
return main
