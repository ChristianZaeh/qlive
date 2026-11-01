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
qlive live F [--port N]       live preview: warm Quarto renders, pushed without reload
qlive preview F [--port N]    cold render once, then re-render on save (M0)
qlive cells F [--once]        execute only the cells that changed
```

### `qlive live`: preview without reloads

For **preview while typing without saving**, install the local
[VS Code extension](editors/vscode/README.md) and run **qlive: Start Live Preview**.
It streams the active document's buffer to a token-authenticated loopback endpoint,
debounces edits by 100 ms, and patches the preview using the warm pipeline. Prose
edits reuse existing cell outputs. Code and front-matter changes require a save.
Only the preview's source document is streamed; included files still come from disk.
This is the initial M3 path, with no claim yet of sub-50-ms keystroke latency.

One cold `quarto render` (with capture) yields the full page, its shell, and Quarto's
pipeline to host warm. Each save then re-renders the body through Quarto's own reader
and filters, adds the heading anchors Quarto's postprocessor would, splices the result
into the shell, and pushes it over the WebSocket; the browser swaps `<main>` in place.

Verified in a browser, with a marker set on `window` before each edit — a patch keeps
it, a reload wipes it:

| edit | path | time | reload? |
|---|---|---:|---|
| body: changed sentence + new paragraph with math | warm | 35 ms | no — marker kept |
| front matter: new title | cold | 2042 ms | yes — marker gone, new title shown |
| six further body edits | warm | 25–34 ms | no — 6 patches, marker kept |

On a ~25-line document at 15% CPU load. For a full chapter expect the bench's warm
pipeline figure, ~80 ms. `quarto preview` takes ~1040 ms per save on an idle machine,
and reloads the page.

The warm path is taken only where it is known to match Quarto. The cold path (a full
render and a reload) is used when:

* **the front matter changed** — the template, title block and metadata depend on it;
* **Quarto's filters reported FATAL** — pandoc exits 0 regardless, so stderr decides;
* **the self-check failed** at cold-render time. The body Quarto fed pandoc must equal
  the document's body; engine preprocessing breaks that, so documents with executable
  cells or includes get a cold render per save instead of a wrong preview.

Known gaps:

* Page scripts that ran at load (anchor.js link icons, code copy buttons, callout
  collapse toggles) do not re-attach to swapped content. Math is re-typeset.
* The whole body is swapped on each save. At 25–80 ms per render that is fine;
  block-level patches are an optimisation, not a requirement.

### Code cells, and variables shared across documents

With code cells, Quarto never executes anything. qlive holds one kernel, runs only
the cells whose state could have changed, and:

* **cold:** writes a notebook with our outputs and renders it with
  `quarto render <doc>.qlive.ipynb --no-execute` (captured), so Quarto renders our
  results and never runs a cell;
* **warm:** turns the outputs into the markdown Quarto's Jupyter engine would hand
  pandoc (`qlive-quarto::engine`, byte-for-byte against a captured render) and runs it
  through the warm pipeline.

The self-check compares our engine markdown with what Quarto made of our notebook on
the cold render; any difference turns the warm path off for that document.

**The recommended way to share variables is a master file that includes its
chapters:**

```
---
title: My thesis
---

{{< include chapters/ch1.qmd >}}

{{< include chapters/ch2.qmd >}}
```

`qlive live master.qmd` takes the order from the include lines and watches every
included file, nested ones too. Because Quarto inlines includes into **one document**,
its chapters share one kernel natively — `quarto render master.qmd` works on its own,
with no order-dependence problem. qlive's expansion (`qlive-core::includes`) follows
Quarto's implementation rather than its documentation, including the non-obvious rule
that **every include path resolves against the master's directory**, nested ones too,
not against the file containing the include line.

Verified against Quarto's own native render of a master with two chapters in a
subfolder, Python cells in each, and a nested include with a decoy file of the same
name next to the includer: what Quarto fed pandoc and what qlive feeds it are
**byte-identical (615 characters)** apart from cell ids, and both used the file in the
master's directory. In the browser, editing the doubly-nested include ran no cells and
patched the page; changing `data` in chapter 1 re-ran chapter 1's cell and chapter 2's
and showed the new sum, without a reload.

`--after` chains *separate* documents through one kernel instead:
`qlive live ch2.qmd --after ch1.qmd`. A cell's key covers every code cell
before it in every earlier document, so editing chapter 1 re-runs chapter 2 and
editing chapter 2 leaves chapter 1 cached.

> **This makes chapters order-dependent.** Quarto gives every document a fresh
> kernel — its own `setup.py` runs `%reset` — so `quarto render ch2.qmd` on its own
> will fail with a `NameError`. A chained book must be built through qlive's notebook
> path, not by Quarto executing chapters independently.

Verified in a browser with two chapters — chapter 1 defines `data` in a 2-second cell,
chapter 2 uses it without defining it — counting real executions from a log every
cell appends to:

| edit | cells that ran | time | reload? |
|---|---|---:|---|
| first run | ch1-slow, ch2-print, ch2-plot | 2087 ms kernel | — |
| prose in chapter 2 | **none** | 54 ms | no |
| plot cell in chapter 2 | **ch2-plot only** | 87–116 ms | no; new figure shown |
| `data` in chapter 1 | ch1-slow, ch2-print, ch2-plot | 2116 ms | no; chapter 2 shows `sum of data: 55` |
| new unrelated cell at the end of chapter 1 | ch1-unrelated, **ch2-print, ch2-plot** | 111 ms | no |

Three bugs only running it found:

* **Quarto's `setup.py` runs `%reset`**, which asks "Proceed (y/[n])?" on stdin. Our
  worker left stdin enabled, so the kernel waited forever for a keypress. nbclient
  executes with `allow_stdin=False`, and IPython treats a missing stdin as yes; the
  worker now does the same for every execution.
* **Re-run figures showed the old image.** A figure is rewritten at the same path, and
  the browser served its cached copy after the swap. Image URLs in the pushed HTML are
  now versioned by a hash of the file's bytes — a new URL exactly when the image
  changed — and not in the markdown, so the self-check is unaffected.
* **A borrow conflict** passing the whole cold state while holding a mutable borrow of
  its worker; fixed by passing only the fields read.

### Which cells run: a dependency graph (M6)

The last row of the table above was the problem with the original rule: adding a cell
that **nothing uses** re-ran both chapter-2 cells, because a cumulative prefix
invalidates everything after an edit — across every later chapter, whether or not it
depends on what changed.

Now each cell runs exactly when its source or something it actually reads changed
(`qlive-core::deps`):

* **Analysis.** Python's own `ast`, in the kernel worker — parsed, never executed —
  gives each cell's defined and read names, scope-aware (lambda parameters and
  comprehension variables stay local; a function body's global reads count as reads
  of the defining cell). Cached by source, so only edited cells are re-parsed.
* **Signatures.** A cell's signature hashes its source with the signatures of the
  cells it reads each name from: a Merkle hash over the dependency graph. Outputs are
  cached by signature.
* **The kernel's real state.** For every name the planner tracks which signature's
  value the kernel holds. If a cell about to run would read the wrong one, its
  definer re-runs first. That makes **hidden state** and **mutation** correct, which
  the prefix rule never was: with `x = 1` / `y = x + 1` / `x = 100`, re-running the
  middle cell restores `x = 1` first; re-running `items.append(…)` restores `items`
  instead of appending twice. Names only a deleted cell defined are removed from the
  kernel, so their readers fail as in a fresh run.
* **Opaque cells** — `exec`, `eval`, `globals()`, star imports, magics — depend on
  and define everything: they run after anything before them changed, and everything
  after a running opaque cell runs.

Verified in a real kernel, two chapters, nine cells, counting executions from a log the
cells write and reading the outputs off the served page:

| edit | cells run | reused | result |
|---|---:|---:|---|
| unrelated new cell in chapter 1 (prefix rule: 3) | **1** | 8 | 60 ms |
| plot cell in chapter 2 | 1 | 8 | 95 ms |
| `data` in chapter 1 (prefix rule: all 8 after it) | **3** | 6 | its cell and the two readers |
| `y = x + 1` → `x + 2`, with a later `x = 100` | 2 | 7 | **`y = 3`**, not 102 |
| `items.append(2)` → `append(3)` | 2 | 7 | **`[1, 3]`**, not `[1, 2, 3]` |

Each run is explained in the log: `ran cell 5 'x = 1': restores 'x' for cell 6`.

Cell errors stay local: the error output appears in the failed cell, downstream
readers get a `DependencyError`, and independent cells continue. Failed definitions
are removed from the kernel so an old or partially updated value cannot be used as
that cell's result. Opaque cells (including syntax errors) conservatively block
later cells, because their dependencies cannot be determined. Fixing the failed
source invalidates its dependent results and lets them run again.

Deliberate limits. Method calls count as mutation only for known mutating methods
(`append`, `sort`, `update`, …) and `inplace=True`, plus assignments into attributes or
subscripts — counting every call would make `df.head()` a writer of `df` and bring the
over-invalidation back. Mutation through other methods or function arguments
(`np.random.shuffle(data)`) and dependencies through files are invisible to name
analysis; declare them with `#| depends: data, other`. Cells are assumed deterministic.

**The warm render is O(document).** Measured with `qlive bench --sections 400`:

| document | Quarto pipeline, warm | plain pandoc, warm | one block |
|---|---:|---:|---:|
| 283 lines | 78 ms | 19 ms | 0.7 ms |
| 2,803 lines | 782 ms | 229 ms | 0.8 ms |

Linear in size, and dominated by Quarto's filters, not by reloading `main.lua` — a
six-line document renders in ~20 ms, so the fixed per-render cost is at most that.
With a master file the whole thesis is *one* document, so every save, prose
included, pays for the whole thesis: about 1.4 s at 5,000 lines by this line. The fix
is rendering only what changed, which is hard for exactly one reason: crossref
numbering, citations and footnotes are document-global.

Not a bottleneck: **the per-cell plumbing.** An empty cell costs 6.3 ms through qlive's
worker and 6.6 ms talking to the kernel directly with `jupyter_client`; the worker hop
is ~0.2 ms. The floor is ipykernel itself, so a Rust Jupyter client would save nothing
measurable.

One-time costs, for completeness: the 2 s cold render on front-matter edits and the
3.2 s kernel boot per session.

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
| **M5b** | cells in the live preview, one kernel across chained documents | **done**: prose 54 ms, one cell ~90 ms, no reloads |
| **M6** | dependency graph instead of prefix invalidation | **done**: a cell runs when its source or what it reads changed; hidden state and mutation handled |
| **M7** | Quarto's HTML postprocessor on a real DOM | in progress: anchors, section ids, copy scaffolds, annotations and Bootstrap content classes ported; full-corpus gate still open |

## Parity against Quarto's test corpus

M7's first slice uses an HTML5 DOM (`qlive-quarto::postprocess`) in both the live
preview and the parity command. It ports heading/figure/table anchor classes,
descendant section heading ids, source-code class hoisting and copy-button scaffolds,
code annotation links/buttons, gutters and definition lists, and Bootstrap classes
for blockquotes, figures, images and tables (including computational tables).
Captured metadata controls `minimal`, `anchor-sections` and `code-copy`; captured
language parameters supply the localized copy-button tooltip. Comparison now uses
DOM serialization and preserves whitespace inside code and preformatted elements.
The captured `has-bootstrap` flag gates Bootstrap rules, including section heading
ids. Column layout, table cell/width processing and appendix processing remain open.
The historical full-corpus results below predate M7 and its stricter comparator.

The initial anchors/copy slice had **29 pass, 17 fail, 4 error** in the first 50
eligible corpus documents. See [the latest M7 sample report](m7-parity-50.md) for
results after the Bootstrap and annotation ports. This is a sample result,
not an updated 469-document parity claim. The report's cause labels are heuristic;
copy-scaffold and anchor labels can also indicate later processing still missing.

`cargo test -p qlive-cli postprocessor_matches_quarto_features_and_themes -- --ignored`
checks authored Bootstrap and annotation fixtures against installed Quarto and verifies
captured flags for `theme: none` and `theme: pandoc`. Those unthemed pages have no
`<main>` and are outside the current live shell's warm path.

`qlive parity <dir>` renders each document with Quarto, renders its body through the
warm path, and compares against Quarto's `<main>`. Documents with code cells, local
filters or extensions are skipped and never executed. On quarto-cli's
`tests/docs/smoke-all` at v1.8.27 (469 documents run, 1,163 s):

| bucket | documents | meaning |
|---|---:|---|
| pass | 179 | warm output equals Quarto's |
| **fail** | **262** | **the preview would differ from Quarto's page** |
| error | 27 | mostly Quarto itself unable to render without executing code; 2 are qlive bugs |
| fallback | 1 | self-check turned warm off: slower, never wrong |

| cause of failure | documents |
|---|---:|
| heading, figure and table anchors | 112 |
| not yet categorised | 63 |
| code-copy wrapper | 36 |
| figure markup | 29 |
| tables, cross-references, appendix, callouts | 22 |

Nearly all of it is Quarto's Deno-side HTML postprocessor, which runs after pandoc and
which the warm path does not run. Its rules are small and deterministic — anchors go on
`h2`–`h6`, `.quarto-figure[id]` and `div[id^=tbl-]`; `data-anchor-id` comes from the
first `h2` (else `h3`, …) in each `section[class^=level]`; every `pre.sourceCode` is
wrapped in `div.code-copy-outer-scaffold` with a copy button — but they need DOM
semantics, which regexes cannot give faithfully. Porting them onto a real DOM, in the
order of the table, is M7.
| **M1** | `pandoc lua` worker, whole-document warm passes | **done**: Quarto's own pipeline warm, 78 ms for 283 lines, parity verified |
| **M2** | warm renders patched into the page, no reload | **done for documents without code cells**: 25–35 ms per save on a small document, ~80 ms expected on a chapter; block-level patching and a 50-doc parity corpus remain |
| **M3** | VS Code buffer streaming, no save needed | initial prose path implemented; includes overlays and sub-50-ms p95 gate remain open |
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

> **Correction — that held for one document, not in general.** Quarto's own test
> corpus says otherwise; see *Parity against Quarto's test corpus* below. The filter
> chain output matches; Quarto's Deno-side HTML postprocessor does much more than
> heading anchors, and qlive reproduces little of it yet.

`quarto_pipeline_renders_a_callout_warm` covers the chain end to end — capture, host,
render twice from one process — and is `#[ignore]`d because it runs a real `quarto
render`; run it with `cargo test -- --ignored`.

`main.lua`'s top level re-executes on every render, but that is at most the ~20 ms a
six-line document takes; the filters' cost grows with the document (see *The warm
render is O(document)*). The output is a body fragment; splicing it into the cold
render's shell is M2.
