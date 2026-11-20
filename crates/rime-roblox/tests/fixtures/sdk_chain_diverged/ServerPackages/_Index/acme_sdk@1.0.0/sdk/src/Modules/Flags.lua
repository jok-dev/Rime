-- The server-side Flags is a DIFFERENT module from the shared one: the
-- shared copy is a client shim, the server copy the real thing. A require
-- in a shared module therefore loads two different files depending on where
-- it runs, and Rime cannot pick one: R0502, naming both.
local Flags = {}

function Flags.get(_name: string): string?
  return nil
end

function Flags.set(_name: string, _value: string) end

return Flags
