--!rime
local thing = { value = 3 }
function thing:double()
    return self.value * 2
end
print(thing:double())
