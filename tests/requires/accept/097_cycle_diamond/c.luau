local function value()
	return 1
end

return table.freeze({ value = value })
