# Paragraph preview benchmark

Debug build; authored 300-section document; 10 unsaved edits.

- Buffer request median: 62.9 ms
- Buffer request p95 (10 samples): 94.2 ms
- Structural edit/full-body request: 383.5 ms
- All 10 paragraph edits delivered one WebSocket block replacement each.
- The heading edit delivered a full body; the source file stayed unchanged.

These timings exclude the configurable 100 ms editor debounce and browser paint.
Run: node editors/vscode/benchmark.js target/debug/qlive.exe
