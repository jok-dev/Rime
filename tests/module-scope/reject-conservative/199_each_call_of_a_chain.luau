--!rime
--~ OPTION[R0312]: on
-- Each call is its own report at its own place: every link of a chain,
-- and a call that is an argument of another. The allowed `require`
-- around them hides nothing.

local Storage = game:GetService("ReplicatedStorage")

local Shapes = require(Storage:WaitForChild("Shared"):WaitForChild("Shapes"))
--~ WARNING[R0312]: `Storage:WaitForChild` is called at module scope
--~ WARNING[R0312]: `Storage:WaitForChild():WaitForChild` is called at module scope

local label = Shapes.describe(Shapes.create("square"))
--~ WARNING[R0312]: `Shapes.describe` is called at module scope
--~ WARNING[R0312]: `Shapes.create` is called at module scope

return { label = label }
