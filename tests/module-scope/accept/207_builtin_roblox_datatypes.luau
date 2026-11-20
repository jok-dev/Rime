--!rime
--!no-exec
--~ OPTION[R0312]: on
-- The built-in `roblox-datatypes` list, in use by default: constructors of
-- Roblox's value types. They make a value and touch nothing else.

local Style = {}

Style.Offset = Vector3.new(0, 4, 0)
Style.Turn = CFrame.Angles(0, math.rad(90), 0) * CFrame.new(Style.Offset)
Style.Accent = Color3.fromRGB(255, 170, 0)
Style.Fade = TweenInfo.new(0.25, Enum.EasingStyle.Quad, Enum.EasingDirection.Out)
Style.Size = UDim2.fromScale(0.5, 0.5)
Style.Padding = UDim.new(0, 8)
Style.Glow = NumberSequence.new({
	NumberSequenceKeypoint.new(0, 0),
	NumberSequenceKeypoint.new(1, 1),
})
Style.Range = NumberRange.new(1, 4)
Style.Typeface = Font.fromEnum(Enum.Font.Gotham)

return Style
