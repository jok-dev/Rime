--!rime
--~ OPTION[R0312]: allow = pcall
-- An allowed `pcall` runs the function it is given on the spot, so that
-- function's body is module scope too. Other function arguments are not
-- entered: nothing says when they run.

local Store = {}

local ok, saved = pcall(function()
	return os.time()
	--~ WARNING[R0312]: `os.time` is called at module scope
end)

Store.Saved = if ok then saved else nil

Store.Later = function()
	return os.time()
end

return Store
