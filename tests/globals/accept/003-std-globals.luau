--!rime
print(string.format("%d", math.floor(2.5)))
local ok, err = pcall(function()
    return typeof(buffer.create(4))
end)
print(ok, err)
