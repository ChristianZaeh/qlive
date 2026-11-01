# qlive for VS Code

Run **qlive: Start Live Preview** from the command palette with a saved `.qmd` or
`.md` file open. The extension starts qlive and opens the browser preview. Typing
updates the preview without saving. The status bar reports updates and when a save
is needed. Use **qlive: Stop Live Preview** to stop its daemon.

Set `qlive.executable` to the built qlive binary (for this checkout,
`C:\dev\qlive\target\debug\qlive.exe`). `qlive.debounceMs` defaults to 100 ms.
Quarto and the usual qlive dependencies must be available. The extension requires
a trusted workspace and has no npm runtime dependencies.

The first version streams the document used to start the preview. Included files
are read from disk; unsaved changes in other tabs are not streamed into the master.
Prose edits reuse existing cell outputs. Save to execute changed code cells, rebuild
front matter, or render documents without a working warm pipeline. Closing the
source tab or the extension stops the daemon. qlive never saves the editor buffer.

Failed cells display errors; their dependent cells are blocked, and unrelated cells
continue. Dependencies use Python name analysis with qlive's documented limits;
an unparseable cell conservatively blocks later cells.

Build and install the local VSIX with `python editors/vscode/package.py`, then
`code --install-extension editors/vscode/qlive-preview-0.1.0.vsix --force`.

Validation:

```
node --check editors/vscode/extension.js
node editors/vscode/smoke.js target/debug/qlive.exe
```

The smoke check runs authored fixtures with installed Quarto and Python/Jupyter.
