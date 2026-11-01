# qlive for VS Code

Click **Preview**, press **Ctrl+Shift+K** (**Cmd+Shift+K** on macOS), or run
**qlive: Preview** with a saved `.qmd` or `.md` file open. The extension starts the
Rust qlive daemon and opens a side-by-side preview panel. Typing
updates the preview without saving. The status bar reports updates and when a save
is needed. The panel toolbar provides refresh, open-in-browser, and stop controls.
Repeated Preview presses reveal the existing session. Closing the panel stops its
daemon and child processes.

Set `qlive.executable` to the built qlive binary (for this checkout,
`C:\dev\qlive\target\debug\qlive.exe`). `qlive.debounceMs` defaults to 100 ms.
Quarto and the usual qlive dependencies must be available. The extension requires
a trusted workspace and has no npm runtime dependencies.

The master and its included files stream unsaved buffers into one preview. Changes
in multiple open chapters are combined; saving one chapter retains the others'
unsaved prose. Discarding/closing an included buffer returns it to the saved source.
Preview from an included chapter reuses the master's session. Prose edits reuse
existing cell outputs. Save to execute changed code cells, rebuild front matter, or
render documents without a working warm pipeline. Closing the master tab or the
extension stops the daemon. qlive never saves the editor buffer.

Use **qlive: Toggle Paper/Web View**, the panel's book icon, or the in-page toggle
to change appearance without reloading or restarting the kernel. Paper view uses a
continuous A4-width sheet, serif text and paper margins. It approximates PDF
appearance; it does not reproduce the PDF engine's exact pagination. The choice is
remembered, and opening the external browser carries over the selected mode.

Failed cells display errors; their dependent cells are blocked, and unrelated cells
continue. Dependencies use Python name analysis with qlive's documented limits;
an unparseable cell conservatively blocks later cells.

Build and install the local VSIX with `python editors/vscode/package.py`, then
`code --install-extension editors/vscode/qlive-preview-0.5.0.vsix --force`.

To route the installed Quarto extension's existing Preview menus and shortcut into
qlive, run `python editors/vscode/integrate_quarto.py <installed-quarto-extension-dir>`
and reload VS Code. This modifies only its contributed menus/keybinding, keeps a
`package.json.qlive-backup`, and leaves other Quarto commands available. The qlive
extension hides its duplicate button when this integration is detected. Run the
script again after Quarto extension updates; pass `--restore` to undo it.

The **Quarto Sources** tree shows the master and nested includes. Clicking a rendered
cell's output opens its producing file and first code line. Links and copy controls
retain their normal actions. Rust also provides insert-cell, next/previous-cell and
new-document commands. Integration redirects the corresponding existing cell controls.

Ordinary single-paragraph edits use a block patch. Structural edits, citations,
math, raw HTML, lists and custom filters use the whole-document path. See the
repository's `paragraph-benchmark.md` for measured transport timings; they exclude
typing debounce and browser paint. Cursor-jump protocol tests passed; the final
interactive cursor-jump check was interrupted by the user.

This is a Rust-backed replacement for the preview workflow. The full authoring
extension port is tracked in [ROADMAP.md](ROADMAP.md); language and authoring tools
are still provided by the installed Quarto extension.

Validation:

```
node --check editors/vscode/extension.js
node editors/vscode/smoke.js target/debug/qlive.exe
```

The smoke check runs authored fixtures with installed Quarto and Python/Jupyter.


## Preview comments and live agents

Click **Comment**, then a preview paragraph, figure or cell. **Enter** saves the
comment; **Shift+Enter** inserts a newline. Comments persist in the project-local
`.qlive/agent-inbox.json` with an agent-readable `agent-inbox.md` projection.
The nearest Quarto project or Git root owns the inbox; standalone files use their
parent directory. Keep private inbox contents out of version control.

Include `@codex` or `@claude` to start that installed, authenticated CLI automatically.
`@agent` uses `qlive.defaultAgent` (initially Codex). Configure
`qlive.codexExecutable` / `qlive.claudeExecutable` when needed; the Claude Code
extension's bundled executable is detected automatically. CLI sign-in remains with
the provider. Ordinary comments start no agent. Tagged requests require the VS Code
preview bridge. Source snapshots, including unsaved included documents, accompany
the request. Requests run serially per project and show queued/working/results.

Agents return an answer or propose one complete source-file edit. Rust restricts
runners to read operations; the bridge applies the proposed edit only when the
current editor text exactly matches the captured snapshot. Newer typing produces
a conflict in the inbox. Edits remain unsaved and undoable; save changed code to
execute it. Resolve cancels pending work. Interrupted requests require a new
comment. Changed or ambiguous preview locations remain in the inbox without a pin.
This does not stream intermediate agent reasoning or execute code before save.

Validation uses fake CLI runners, not paid model calls:

```
node editors/vscode/agent-smoke.js target/editor-next/debug/qlive.exe
node editors/vscode/agent-editor-test.js
```

The optional comment DOM check exercises click anchoring, Enter/Shift+Enter,
text escaping, pins and resolve without a browser:

```
pnpm --dir target/comment-dom add jsdom
node editors/vscode/comment-dom-test.js
```

Create `target/comment-dom` first. Visual browser verification remains separate.
