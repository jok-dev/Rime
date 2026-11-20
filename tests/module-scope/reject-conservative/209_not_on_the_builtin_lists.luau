--!rime
--~ OPTION[R0312]: on
-- Where the built-in lists stop: making an instance, reading the clock,
-- waiting for or looking up an instance, starting a thread, writing to
-- the output. A project that wants one of these says so in `allow`.

local Storage = game:GetService("ReplicatedStorage")

local Panel = {}

Panel.Frame = Instance.new("Frame")
--~ WARNING[R0312]: `Instance.new` is called at module scope

Panel.Started = os.clock()
--~ WARNING[R0312]: `os.clock` is called at module scope

Panel.Opened = DateTime.now()
--~ WARNING[R0312]: `DateTime.now` is called at module scope

Panel.Assets = Storage:WaitForChild("Assets")
--~ WARNING[R0312]: `Storage:WaitForChild` is called at module scope

Panel.Template = Storage:FindFirstChild("Template")
--~ WARNING[R0312]: `Storage:FindFirstChild` is called at module scope

Panel.Refresh = coroutine.wrap(function() end)
--~ WARNING[R0312]: `coroutine.wrap` is called at module scope

warn("panel loaded")
--~ WARNING[R0312]: `warn` is called at module scope

return Panel
