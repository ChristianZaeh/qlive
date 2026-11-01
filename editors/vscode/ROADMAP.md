# Quarto editor replacement

Reference: installed `quarto.quarto` 1.139.0 (48 contributed commands). Rendering,
dependency planning, buffer processing, and preview transport live in Rust. VS Code
requires a small JavaScript adapter for its editor/command/webview APIs. Python
cells still run in a Python kernel.

| Workflow | State | Next gate |
|---|---|---|
| Existing Preview button and Ctrl/Cmd+Shift+K | Implemented through reversible local manifest integration | Verified by clicking the original button and invoking the shortcut in VS Code |
| Side-by-side preview, refresh, browser, stop | Implemented | Verified panel rendering, unsaved prose, unchanged disk source and daemon stop |
| Unsaved source document | Implemented for prose; saved code outputs reused | Stream unsaved included chapters; keep the latest accepted buffer visible |
| Cell execution and error isolation | Rust planner/kernel orchestration implemented on save | Editor commands for current/above/below cells and explicit execution |
| Insert cell, navigate cells | Rust source-range queries and editor commands implemented | Broader authoring parity fixtures |
| New document/project/presentation, convert, render formats/projects | New-document template implemented in Rust; other Quarto commands remain available | Rust commands delegating cold work to Quarto CLI |
| Nested files and output-to-source | Include tree and mapped cell output navigation implemented | Final interactive cursor-jump verification; unsaved included buffers |
| Paragraph updates | Conservative block rendering and patch transport implemented | Broader eligible subset and keystroke-to-paint latency gate |
| Syntax, outline, completions, diagnostics, formatting, assist | Original Quarto extension remains the provider | Specify and implement a Rust language service; preserve existing authoring behavior |
| Visual editor, citations/Zotero, math/diagram previews | Original Quarto extension remains the provider | Separate feature ports and UI parity fixtures |

The preview port is usable now. This table does not claim that all Quarto authoring
features have been reimplemented in Rust. Local manifest integration redirects
preview controls without copying the upstream extension's implementation.
