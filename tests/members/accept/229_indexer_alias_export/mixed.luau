--!rime
-- Named fields beside the indexer, optional, and exported.
export type Settings = { name: string, [string]: number | string }

local mixed: Settings? = { name = "mixed" }

return mixed
