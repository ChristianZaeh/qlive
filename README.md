# qlive

Instant Quarto preview. Quarto keeps the cold path — templates, themes, Lua filters,
bibliography, formats. qlive owns the hot loop: warm engines, block-level diffing, and
patches pushed to the browser instead of page reloads.

**Status: M0 scaffold. Compiles clean; 7 unit tests pass.** The tokio-linked targets
need the MSVC toolchain on Windows — see *Building* below.

## Why

Measured on the development machine (Quarto 1.8.27, pandoc 3.6.3, typst 0.13.0,
Windows 11, 283-line document):

| path | cost |
|---|---:|
| `quarto preview` save → browser reload | 1040 ms |
| `quarto render --to html` one shot | 1906 ms |
| `pandoc.exe` one-shot | 74 ms |
| warm pandoc, whole document | ~16 ms |
| warm pandoc, **one block** | **0.52 ms** |
| `pandoc server` HTTP, one block | 1.2 ms |
| `typst watch` rebuild, as typst reports it | **3.1 ms** |
| any Python cell, on top of the above | +4300 ms |

Almost all of every Quarto save is process startup and pipeline setup; the actual
rendering of a mid-size document is ~16 ms, and a single block is half a millisecond.
That gap — roughly 2000× — is the project. A document with Python cells is a separate
problem: a flat ~4.3 s of jupyter engine regardless of what the cells do, which is
what the kernel pool in M5 addresses.

`qlive bench` reproduces this table wherever it runs, so the premise stays a claim you
can check rather than one you have to trust.

**Run it on an idle machine.** Every number here is CPU-bound and measured by wall
clock. A run taken next to a game and a browser came out 2–16× worse across the board
(55 s for the cold `quarto render`, 10.8 s for a one-shot typst compile), which says
nothing about either tool.

### Bench result

`qlive bench`, 283-line fixture, 121 blocks, 20 warm iterations, **idle machine**:

| step | ms |
|---|---:|
| quarto render --to html (cold, Deno included) | 1935.0 |
| pandoc one-shot, whole doc (process start) | 71.6 |
| pandoc server, whole doc | 23.4 |
| pandoc server, one block | 1.1 |
| **pandoc lua worker, whole doc** | **19.3** |
| **pandoc lua worker, one block** | **0.7** |
| **quarto pipeline (reader + filters), warm, whole doc** | **78.3** |
| typst compile, one-shot (process start) | 835.7 |
| typst watch, edit -> new PDF (end to end) | 135.8 |
| &nbsp;&nbsp;of which typst reports as compile | **5.2** |

CPU load was 17% before the run and 10% after. The typst one-shot figure is noisy
between runs (89 ms in the previous one).

**Quarto's own pipeline renders the whole 283-line document warm in 78 ms**, against
1935 ms for `quarto render` — 25× — with output that matches Quarto's (see *Capture and
replay*). Quarto's filters account for ~59 ms of it on top of plain pandoc.

The `pandoc lua` worker beats `pandoc server` on both sizes and, unlike the sandboxed
server, can host Lua filters — so it is the path forward. Inside Lua, pandoc's own
`read` + `write` for one block measures 0.52 ms; 0.7 ms is the round trip from Rust.

> A one-word edit dirties 1 of 121 blocks.

That last line is the premise, now measured rather than asserted: the hot path has one
block to re-render, at ~1 ms, in place of the ~1.9 s cold render it replaces. The
132 ms end-to-end against typst's own 3.1 ms is file-watch latency, which is exactly
the argument for M4 using typst as a library instead of a subprocess.

For scale, `quarto preview` save → browser reload on the same idle machine is
**1040 ms** (1046 / 1038 / 1042 over three edits). That is the number to beat.

The same bench on a machine running a game gave 14270 ms for the cold render and
4.9–10.9 ms per block — 3–7× worse throughout. The ratios survived; nothing else did.

One finding worth keeping: **`typst watch` sometimes misses an edit outright.** Twice
in four runs the process stayed alive, its last line a successful startup compile, and
the output never moved for a full minute. `TypstWatch::time_edit` therefore re-writes
the source and waits again rather than trusting a single event. A preview built on a
file watcher inherits this; one built on typst-as-a-library does not.

## Commands

```
qlive doctor                  which quarto / pandoc / typst / python were found
qlive bench [--doc F]         print the latency table for this machine
qlive preview F [--port N]    cold render once, then re-render on save
qlive cells F [--once]        execute only the cells that changed
```

### `qlive cells`: per-cell execution

Quarto cannot do this. `cache: true` is document-level — editing one cell re-runs every
cell — and `freeze: auto` re-executes everything when you edit a *paragraph*. Measured
on a two-cell document where each cell sleeps 2 s, counting actual executions:

| scenario | executed | reused | kernel | render | total | cells that ran |
|---|---:|---:|---:|---:|---:|---|
| first pass | 2 | 0 | 4026 ms | 1883 ms | 5911 ms | A, B |
| last cell edited | **1** | 1 | 2015 ms | 1891 ms | 3908 ms | B only |
| prose only | **0** | 2 | 0 ms | 1779 ms | **1780 ms** | none |
| first cell edited | 2 | 0 | 4020 ms | 1894 ms | 5915 ms | A, B |

Quarto on the same document: 10445 ms for the one-cell edit (both cells), 3897 ms for
the prose edit with `cache: true`, 10289 ms without. The saving grows with how much
upstream work there is — here cell A is only 2 s.

Image outputs survive: a matplotlib figure arrives as `display_data` and Quarto writes
it out as `…_files/figure-html/cell-2-output-1.png`, same as it would normally.

**How it stays correct.** A cell's key is `hash(its source + every preceding code
cell's source)`, so prose never invalidates anything and editing a cell invalidates it
*and everything after it* — those cells saw different state. Reuse is allowed only for
the common prefix of the document's keys and the keys this kernel actually executed,
which is why the kernel is held for the life of the command: skipping a cell is sound
only while the variables it defined are still live. A cold kernel shares no prefix and
runs everything.

Known gap: the notebook is assembled by qlive rather than by `quarto convert`, and
there is no parity test yet against `quarto render doc.qmd` on real documents. Output
lands next to the source as `doc.qlive.html`.

`qlive preview` is deliberately dumb in M0: it still runs a full `quarto render` per
save, but the server, the socket and the browser tab survive, so nothing restarts and
the page keeps its scroll position. That alone removes the Deno tax from every save.

## Layout

```
crates/qlive-core      block splitter, hashing, dirty-block counting, tool discovery
crates/qlive-pandoc    warm pandoc: `pandoc server` now, `pandoc lua` worker at M1
crates/qlive-quarto    cold renders, output paths, shell extraction (M2)
crates/qlive-typst     typst subprocess now, typst-as-library at M4
crates/qlive-server    axum static server + /qlive/ws patch channel + client.js
crates/qlive-cli       the `qlive` binary
```

## Building

```
cargo test --workspace
```

```
cargo run -p qlive-cli -- doctor
```

```
cargo run -p qlive-cli --release -- bench
```

Dependency versions are pinned to majors that match the APIs used here: `axum 0.7`
(`Message::Text(String)`, `/*path` wildcard routes), `notify 6`
(`recommended_watcher`), `ureq 2` (`send_string`). If you bump either major, expect
the server and watcher to need edits.

### Windows: use the MSVC toolchain

**`stable-x86_64-pc-windows-gnu` as rustup ships it cannot build this project.** The
findings, so nobody repeats them:

* With no linker at all, rustc silently picks up Git Bash's `/usr/bin/link.exe` —
  which is GNU coreutils' hard-link tool — and reports
  `link: extra operand ... Try 'link --help'` from random build scripts.
* The windows-gnu toolchain's `self-contained` directory holds `dlltool.exe`, `ld.exe`
  and `x86_64-w64-mingw32-gcc.exe` but **no `as.exe`**, so `dlltool` fails with a bare
  `CreateProcess` error when `windows-sys` asks for import libraries.
* Pointing rustc at LLVM's dlltool instead (`-Cdlltool=…/llvm-dlltool.exe`, created by
  copying the multicall `llvm-ar.exe`) makes everything *compile* — and produces
  binaries that die at startup with `STATUS_ACCESS_VIOLATION`. Verified with a
  five-line tokio program, so it is the import libraries, not this code.

Status on that toolchain: `cargo check --workspace --all-targets` is clean, and the
crates without tokio in their tree pass their tests (`qlive-core` 6/6, `qlive-quarto`
1/1). Anything linking tokio cannot run.

So, on Windows:

```
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

```
rustup default stable-x86_64-pc-windows-msvc
```

MSVC's `link.exe` handles raw-dylib imports itself, so no `dlltool` is involved. It is
also what M5 will need, since the ZeroMQ crates want a C compiler. The alternative, if
you would rather stay on GNU, is a real mingw-w64 binutils (MSYS2), which provides a
working `as` and `dlltool`.

## Roadmap

| # | Scope | Gate |
|---|---|---|
| **M0** | scaffold: bench + cold render + watch + reload | **done** |
| **M5a** | per-cell execution (`qlive cells`) | **done**: cell edit runs one cell, prose edit runs none |
| **M1** | `pandoc lua` worker, whole-document warm passes | **done**: Quarto's own pipeline warm, 78 ms for 283 lines, parity verified |
| **M2** | block rendering + DOM patching, `data-qlive-id` | p50 under 25 ms; HTML identical to `quarto render` on a 50-doc corpus |
| **M3** | VS Code buffer streaming, no save needed | keystroke → pixel under 50 ms p95 |
| **M4** | typst as a library, per-page SVG patches | under 30 ms per edit |
| **M5** | Jupyter kernel pool + per-chunk cache | prose edit = 0 executions |

The hard part is not speed. Quarto's Lua filter runtime is undocumented and
version-coupled, so M1 onwards must pin a Quarto version and gate every commit on a
corpus test that diffs qlive's HTML against `quarto render`'s. Fast and approximately
right is worse than slow.

### What the filter chain needs (probed)

Running Quarto's own filters under a plain pandoc works, and needs all of:

* `--data-dir <quarto>/share/pandoc/datadir` — its `init.lua` defines `param` and the
  `quarto.*` globals, and reads `QUARTO_FILTER_PARAMS` unconditionally
* `--from <quarto>/share/filters/qmd-reader.lua` — the filters consume
  `quarto_pandoc_reader_opts` from metadata, which only Quarto's reader injects, so
  `-f markdown` fails inside the `Meta` handler
* `--lua-filter main.lua`, with the filters directory resolvable for `require`
  (`modules/…` is relative)
* env: `QUARTO_SHARE_PATH`, `QUARTO_FILTER_DEPENDENCY_FILE`, and
  `QUARTO_FILTER_PARAMS` = base64 JSON needing at least `quarto-filters.entryPoints`,
  `results-file`, `active-filters`

Each missing piece surfaces as one precise Lua error, so the set is enumerable — five
layers in six attempts. But that params object is Quarto's internal contract, so the
robust approach is to **capture a real render's pandoc invocation** and replay it warm,
rather than reconstruct it by hand.

### Capture and replay (done, verified)

Quarto has this built in: set `QUARTO_CAPTURE_RENDER_COMMAND=<dir>` and `quarto
render` writes `<dir>/render-command.json` — the pandoc binary, args, cwd and env —
right before it runs pandoc, copying its temp files beside it. The real invocation is
simpler than the hand-built probe:

```
pandoc +RTS -K512m -RTS --defaults <quarto-defaults.yml> <quarto-input.md>
       --metadata-file <quarto-metadata.yml> --data-dir <quarto>/share/pandoc/datadir
```

The defaults file carries `from: qmd-reader.lua`, `filters: [main.lua]`, the patched
template and the writer options; `QUARTO_FILTER_PARAMS` carries 72 keys. Replaying it
outside Quarto works, with four things learned the hard way:

* **Quarto's process env is required, not just the two vars it passes explicitly.**
  Without `QUARTO_SHARE_PATH` (and the other `QUARTO_*` from the capture's `ourEnv`),
  `init.lua` cannot extend its module path and fails on `require('_format')`.
* **Session files are not captured.** `include-before/after-body` and
  `notebook-context` live in Quarto's session temp dir, which is deleted
  unconditionally after every render; the capture only rebases the render's own temp
  dir. They feed only the page shell, so empty placeholders are fine for the hot path.
* **Exit code 0 does not mean success.** A FATAL filter error still exits 0 and leaves
  a stub; check stderr and the output.
* **The capture dumps the whole process environment** into `ourEnv` — tokens included.
  Keep only `QUARTO_*` before storing or sharing one.

Parity, on a document with a title block, a section and a callout: the replayed body
equals the content of Quarto's `<main>` except for heading anchors
(`class="anchored" data-anchor-id=…`, added by Quarto's postprocessor) and attribute
quote style (Deno's DOM re-serialisation). The callout markup — which only Quarto's
filters produce — is identical. Everything outside `<main>` (layout, scripts,
dependencies) is the postprocessor's and comes from the one cold render.

### Hosted warm (done, verified)

`LuaWorker::start_quarto` loads the captured pipeline into one long-lived `pandoc lua`
process, and `render_quarto` runs it per edit. Three things had to be worked around,
each forced by a specific error:

* **`PANDOC_STATE.user_data_dir` is nil** without `--data-dir`, and `init.lua` builds
  its module paths from it. The field is read-only, so the worker shadows the global
  with a table that sets it and falls through to the real state for everything else.
* **`main.lua` must run in `_G`.** `pandoc.utils.run_lua_filter` defaults to its own
  environment, but `_utils.lua` (loaded by `init.lua`) calls globals that `main.lua`
  defines — `quarto_assert` was the first to fail. Under real pandoc they share one.
* **`pandoc.read` rejects custom readers**, so the worker loads `qmd-reader.lua` and
  calls its `Reader` directly. `FORMAT` and `PANDOC_WRITER_OPTIONS` are set by hand,
  because `pandoc lua` sets neither.

Parity, checked against Quarto's own `<main>` on a document with a title block, a
section and a callout: after removing exactly three known differences the output is
**identical**, character for character. The three are the title block (pandoc's
template, i.e. the page shell — the worker renders a body fragment), heading anchors
(Quarto's postprocessor) and attribute quote style (Deno's re-serialisation).

`quarto_pipeline_renders_a_callout_warm` covers the chain end to end — capture, host,
render twice from one process — and is `#[ignore]`d because it runs a real `quarto
render`; run it with `cargo test -- --ignored`.

Not done: `main.lua`'s top level re-executes on every render, which is likely part of
the ~59 ms filter cost and the obvious next optimisation. And the output is a body
fragment; splicing it into the cold render's shell is M2.
