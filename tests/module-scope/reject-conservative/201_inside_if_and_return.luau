--!rime
--~ OPTION[R0312]: on
-- An `if` is not work, but its condition and what it holds are checked,
-- and so is the value the module returns.

local Config = {}

local DEBUG = false

local function seal(config)
	return table.freeze(config)
end

if os.getenv("MODE") == "debug" then
--~ WARNING[R0312]: `os.getenv` is called at module scope
	DEBUG = true
	print("debug mode")
	--~ WARNING[R0312]: `print` is called at module scope
end

do
	local stamp = os.time()
	--~ WARNING[R0312]: `os.time` is called at module scope
	Config.Stamp = stamp
end

Config.Debug = DEBUG

return seal(Config)
--~ WARNING[R0312]: `seal` is called at module scope
