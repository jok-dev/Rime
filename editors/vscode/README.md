# Rime for VS Code

Talks to the `rime` CLI in one of two modes, picked by `rime.mode`.

## Syntax highlighting

The extension bundles a Luau TextMate grammar (`syntaxes/luau.tmLanguage.json`,
scope `source.luau`) and registers the `luau` language for `.luau` and `.lua`,
so Luau files — and hover/signature popups — are highlighted with no other
extension installed. If you also have a Lua/Luau grammar extension (e.g.
`undermywheel.roblox-lua`), disable it: both declaring `source.luau` and
claiming `.lua`/`.luau` will conflict. The grammar is JohnnyMorganz's
`Luau.tmLanguage` (MIT — see `syntaxes/luau.tmLanguage.LICENSE`).
The file icon is the "moon" icon of Feather (MIT — see
`icons/luau.svg.LICENSE`).

## Modes

- **`lsp` (default)** — starts `rime lsp` as a long-lived language server via
  `vscode-languageclient`: warm state, check-on-type, quick fixes (the
  lightbulb), and the server registers its own file watching, so nothing on
  the extension side re-checks on disk changes. `rime.check` sends the
  server's `rime.recheck` command; `rime.restartServer` restarts the client.
  `rime.debounceMs` is passed once at startup as `initializationOptions`, so
  a change to it needs a window reload.
- **`spawn` (tier-1 fallback)** — set `rime.mode` to `"spawn"` to fall back
  to the original per-save integration: on every `.luau`/`.lua` save, or any
  on-disk change to a source file, `rime.toml`, a Rojo project file or
  `.luaurc` (git checkout, Rojo, another editor), it runs
  `rime check --format json .` from the first workspace folder and publishes
  the diagnostics. No LSP, no code actions. `rime.checkOnSave` and
  `rime.watchFiles` apply only in this mode.

Either mode runs whatever `rime.path` points at — replace the executable
with a newer release, or rebuild it (`cargo build --release`), and it takes
effect on the next check/restart, no extension changes needed. `rime.mode`
changes need a window reload (`Developer: Reload Window`).

### Smoke test (lsp mode)

1. Set `rime.path` to a released executable, or to a **release** build of
   your own, e.g. `<repo>\target\release\rime.exe`
   (`cargo build --release` first if it isn't there yet).
2. Open a Luau workspace and check the "Rime" output channel: after startup
   it should log a line like `rime: N file(s) republished in …`.
3. Edit a module that many files `require`, save, and watch diagnostics in
   consumer files update.
4. Trigger a quick fix on an R0707 (`"it\'s"`) — the lightbulb should
   offer to remove the backslash.
5. `git checkout` a different branch and confirm a full recheck runs
   (watched via server-side `workspace/didChangeWatchedFiles`, no
   extension-side watcher needed).
6. Delete a file that other files `require` and confirm R0501 appears in
   the consumers.

## Setup

1. Get the `rime` executable: download the file for your system from the
   [releases](https://github.com/jok-dev/Rime/releases), as the
   [Install](https://github.com/jok-dev/Rime#install) section of the main
   README describes.
   Building it yourself works too: `cargo build --release` in the Rime repo.
2. Put it on PATH as `rime` and leave the default, or set `rime.path` in
   VS Code settings to the file, e.g. `C:\\tools\\rime.exe` or
   `<repo>\\target\\release\\rime.exe`.
3. Open a workspace whose root (or an ancestor of it) holds your
   `rime.toml` — in both modes discovery walks up from the first workspace
   folder, never from the server's or the extension host's cwd.

## Build / install

```
npm install
npm run compile          # tsc → out/
npm test                 # mapping unit tests (plain node, no VS Code host)
npx @vscode/vsce package --allow-missing-repository
code --install-extension rime-vscode-0.1.3.vsix
```

The `.vsix` is named after `version` in `package.json`.

Or open `editors/vscode` in VS Code and F5 for a dev host.

## Settings

- `rime.mode` — `"lsp"` (default) or `"spawn"`. Reload the window after changing.
- `rime.path` — rime executable (default `rime`).
- `rime.checkOnSave` — (spawn mode only) default `true`.
- `rime.watchFiles` — (spawn mode only) also re-check on out-of-editor file changes (default `true`).
- `rime.debounceMs` — quiet period after the last change before a run in spawn mode, or the server's debounce window in lsp mode (default `100`; lsp mode needs a window reload after changing this).

Exit code 1 from the CLI means "diagnostics found", not failure.
